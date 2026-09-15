//! Состояние приложения: страницы, параметры, устройство.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use scanner_core::imageproc::{self, PageEdit};
use scanner_core::i18n::Lang;
use scanner_core::{DeviceCapabilities, ScanOptions, ScanProfile, ScannerDevice, Store};

/// Загрузка изображения в текстуру GDK БЕЗ gdk-pixbuf.
///
/// В минимальных сборках (РЕД ОС, контейнеры) gdk-pixbuf может не иметь
/// декодеров PNG/JPEG — тогда thumbnails и предпросмотр были бы пустыми.
/// Декодируем сами (image crate) и строим MemoryTexture напрямую.
pub fn load_texture(path: &Path) -> Option<gtk4::gdk::Texture> {
    let img = imageproc::load(path).ok()?;
    let (w, h) = (img.width() as i32, img.height() as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    let stride = (w as usize) * 4;
    let rgba = img.into_rgba8().into_raw();
    let bytes = gtk4::glib::Bytes::from_owned(rgba);
    let memory = gtk4::gdk::MemoryTexture::new(
        w,
        h,
        gtk4::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    Some(gtk4::gdk::Texture::from(memory))
}

/// Одна страница документа.
#[derive(Debug, Clone)]
pub struct PageItem {
    /// Исходный файл (TIFF/PNG с устройства или импортированный).
    pub source: PathBuf,
    /// Правки пользователя (поворот, обрезка, фильтр).
    pub edit: PageEdit,
}

/// Состояние доступности устройства (последняя проверка).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DevStatus {
    Unknown,
    Ok,
    Down,
}

/// Фаза ручного дуплекса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuplexPass {
    /// Обычное сканирование.
    None,
    /// Первый проход (лицевые стороны) выполняется.
    Front,
    /// Лицевые готовы, ждём подтверждения переворота стопа.
    AwaitingFlip,
    /// Второй проход (оборотные стороны) выполняется.
    Back,
}

pub struct AppState {
    pub lang: Lang,
    pub store: Option<Store>,
    pub devices: Vec<ScannerDevice>,
    pub device: Option<ScannerDevice>,
    pub caps: Option<DeviceCapabilities>,
    pub options: ScanOptions,
    pub pages: Vec<PageItem>,
    pub selected: Option<usize>,
    pub scanning: bool,
    pub profiles: Vec<ScanProfile>,
    /// Индекс последнего выбранного устройства (по device_name).
    pub last_device_name: Option<String>,
    /// Статусы устройств по device_name (для индикатора в комбобоксе).
    pub dev_status: HashMap<String, DevStatus>,
    /// Ручной дуплекс: сканировать в два прохода с переворотом стопа.
    pub duplex_manual: bool,
    /// Текущая фаза дуплекса.
    pub duplex_pass: DuplexPass,
    /// Смещение лицевых страниц в pages (начало пакета первого прохода).
    pub duplex_front_start: usize,
    /// Сколько лицевых страниц отсканировано.
    pub duplex_front_count: usize,
    /// Пропускать пустые страницы.
    pub skip_blank: bool,
    /// Порог пустоты: доля чернил (0.01 = 1 %).
    pub blank_threshold: f32,
    /// Предпросмотр устройства выполняется.
    pub previewing: bool,
    /// Наблюдаемая папка: включена и путь.
    pub watched_enabled: bool,
    pub watched_dir: Option<PathBuf>,
    /// Уже импортированные файлы наблюдаемой папки.
    pub watched_seen: HashSet<PathBuf>,
    /// Шаблон имени файла экспорта.
    pub name_template: String,
    /// Команда подписи PDF (ГОСТ, внешний инструмент контура).
    pub sign_cmd: String,
    /// Подписывать PDF после экспорта.
    pub sign_enabled: bool,
}

impl AppState {
    pub fn load() -> Self {
        let store = Store::open_default().ok();
        let get = |key: &str| -> Option<String> {
            store.as_ref().and_then(|s| s.get_setting(key))
        };
        let lang = get("lang")
            .map(|v| Lang::from_code(&v))
            .unwrap_or(Lang::Ru);
        let last_device_name = get("device");

        let profiles = store
            .as_ref()
            .and_then(|s| s.list_profiles().ok())
            .unwrap_or_default();

        let duplex_manual = get("duplex_manual").map(|v| v == "1").unwrap_or(false);
        let skip_blank = get("skip_blank").map(|v| v == "1").unwrap_or(false);
        let blank_threshold = get("blank_threshold")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.01)
            .clamp(0.001, 0.05);
        let watched_enabled = get("watched_enabled").map(|v| v == "1").unwrap_or(false);
        let watched_dir = get("watched_dir").map(PathBuf::from);
        let name_template = get("name_template")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| scanner_core::naming::DEFAULT_TEMPLATE.into());
        let sign_cmd = get("sign_cmd").unwrap_or_default();
        let sign_enabled = get("sign_enabled").map(|v| v == "1").unwrap_or(false);

        AppState {
            lang,
            store,
            devices: Vec::new(),
            device: None,
            caps: None,
            options: ScanOptions::default(),
            pages: Vec::new(),
            selected: None,
            scanning: false,
            profiles,
            last_device_name,
            dev_status: HashMap::new(),
            duplex_manual,
            duplex_pass: DuplexPass::None,
            duplex_front_start: 0,
            duplex_front_count: 0,
            skip_blank,
            blank_threshold,
            previewing: false,
            watched_enabled,
            watched_dir,
            watched_seen: HashSet::new(),
            name_template,
            sign_cmd,
            sign_enabled,
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        if let Some(store) = &self.store {
            let _ = store.set_setting(key, value);
        }
    }

    /// Сохранённые в БД устройства (добавленные вручную).
    pub fn saved_devices(&self) -> Vec<ScannerDevice> {
        self.store
            .as_ref()
            .and_then(|s| s.list_devices().ok())
            .unwrap_or_default()
    }

    /// Обновить профили из БД.
    pub fn reload_profiles(&mut self) {
        if let Some(store) = &self.store {
            self.profiles = store.list_profiles().unwrap_or_default();
        }
    }
}

pub type SharedState = Rc<RefCell<AppState>>;

/// Рендер страницы с применёнными правками во временный файл.
/// Если правок нет — возвращается исходный файл.
pub fn render_edited(source: &Path, edit: &PageEdit, out: &Path) -> scanner_core::Result<PathBuf> {
    if edit.is_identity() {
        return Ok(source.to_path_buf());
    }
    if out.exists() {
        return Ok(out.to_path_buf());
    }
    let img = imageproc::load(source)?;
    let img = imageproc::apply_edit(img, edit);
    imageproc::save(&img, out, 95)?;
    Ok(out.to_path_buf())
}

/// Эскиз страницы для галереи.
///
/// Оптимизация: сначала уменьшаем исходник (все правки PageEdit — доли/углы,
/// они не зависят от масштаба), потом применяем правки. Экономит ~100x на
/// сканах 600 DPI: рендер эскиза занимает миллисекунды, а не секунды.
pub fn render_thumb(source: &Path, edit: &PageEdit, out: &Path) -> scanner_core::Result<()> {
    const PREVIEW_SIDE: u32 = 720;
    let img = imageproc::load(source)?;
    let img = imageproc::downscale(img, PREVIEW_SIDE);
    let img = imageproc::apply_edit(img, edit);
    let thumb = imageproc::thumbnail(&img, 360);
    imageproc::save(&thumb, out, 95)?;
    Ok(())
}

/// Имя временного файла с учётом правок (кэш).
pub fn edited_cache_path(source: &Path, edit: &PageEdit, suffix: &str) -> PathBuf {
    let crop_key = edit
        .crop
        .map(|c| format!("{:.3}_{:.3}_{:.3}_{:.3}", c.left, c.top, c.right, c.bottom))
        .unwrap_or_else(|| "none".into());
    let key = format!(
        "{:?}_{}_{}_{:.1}_{}_{}_{}_{}",
        source.file_name().unwrap_or_default(),
        edit.rotation_key(),
        crop_key,
        edit.deskew,
        edit.enhance,
        std::process::id(),
        suffix,
        chrono::Local::now().format("%Y%m%d"),
    );
    let name = scanner_core::device::short_id(&key);
    scanner_core::config::tmp_scan_dir().join(format!("edit-{name}-{suffix}.png"))
}

/// Вычислить авто-фикс (угол + рамка) для страницы в фоне.
/// База — исходник с применённым поворотом пользователя, без обрезки.
pub fn compute_autofix(source: &Path, rotation: scanner_core::imageproc::Rotation) -> scanner_core::Result<(f32, Option<scanner_core::imageproc::CropRect>)> {
    let img = imageproc::load(source)?;
    let rotated = match rotation {
        scanner_core::imageproc::Rotation::Deg0 => img,
        scanner_core::imageproc::Rotation::Deg90 => img.rotate90(),
        scanner_core::imageproc::Rotation::Deg180 => img.rotate180(),
        scanner_core::imageproc::Rotation::Deg270 => img.rotate270(),
    };
    Ok(imageproc::auto_fix(&rotated))
}
