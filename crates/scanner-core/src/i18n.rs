//! Локализация RU/EN без внешних зависимостей.
//!
//! Все строки собраны в одной функции `t()`: ключ -> {ru, en}.
//! Такой подход проще gettext и не требует msgfmt при сборке RPM.
//!
//! Внешние переводы: файлы `<код языка>.json` (плоская карта
//! `{"ключ": "перевод"}`) из каталогов
//! `$SCANNER_APP_L10N_DIR`, `~/.local/share/scanner-app/l10n/` и
//! `/etc/scanner-app/l10n/` перекрывают встроенные строки. Новый язык
//! добавляется без перекомпиляции: достаточно `de.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ru,
    En,
}

pub const ALL_LANGS: [Lang; 2] = [Lang::Ru, Lang::En];

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Lang::Ru => "Русский",
            Lang::En => "English",
        }
    }

    pub fn from_code(code: &str) -> Lang {
        match code {
            "en" => Lang::En,
            _ => Lang::Ru,
        }
    }
}

/// Загрузить внешние переводы из каталога (все `*.json`).
/// Внутренняя функция — для юнит-тестов и статического кэша.
fn load_dir(dir: &std::path::Path, tables: &mut HashMap<String, HashMap<String, String>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(code) = p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) else { continue };
        tables.entry(code).or_default().extend(map);
    }
}

fn external_tables() -> &'static Option<HashMap<String, HashMap<String, String>>> {
    static EXTERNAL: OnceLock<Option<HashMap<String, HashMap<String, String>>>> = OnceLock::new();
    EXTERNAL.get_or_init(|| {
        let mut tables = HashMap::new();
        if let Ok(d) = std::env::var("SCANNER_APP_L10N_DIR") {
            load_dir(std::path::Path::new(&d), &mut tables);
        }
        load_dir(&crate::config::data_dir().join("l10n"), &mut tables);
        load_dir(std::path::Path::new("/etc/scanner-app/l10n"), &mut tables);
        if tables.is_empty() { None } else { Some(tables) }
    })
}

/// Каталог пользовательских переводов (для пункта меню «Папка переводов»).
pub fn user_l10n_dir() -> PathBuf {
    crate::config::data_dir().join("l10n")
}

/// Перевод строки по ключу. Внешние JSON перекрывают встроенные значения.
pub fn t(lang: Lang, key: &str) -> String {
    if let Some(tables) = external_tables() {
        if let Some(map) = tables.get(lang.code()) {
            if let Some(s) = map.get(key) {
                return s.clone();
            }
        }
    }
    let (ru, en) = match key {
        // --- Общие ---
        "app.title" => ("Сканер документов", "Document Scanner"),
        "app.ok" => ("Готово", "Done"),
        "app.cancel" => ("Отмена", "Cancel"),
        "app.close" => ("Закрыть", "Close"),
        "app.apply" => ("Применить", "Apply"),
        "app.save" => ("Сохранить", "Save"),
        "app.delete" => ("Удалить", "Delete"),
        "app.error" => ("Ошибка", "Error"),
        "app.retry" => ("Повторить", "Retry"),

        // --- Устройства ---
        "device.title" => ("Сканер", "Scanner"),
        "device.none" => ("Сканер не выбран", "No scanner selected"),
        "device.not_found" => ("Сканеры не найдены", "No scanners found"),
        "device.add" => ("Добавить", "Add"),
        "device.refresh" => ("Обновить список", "Refresh list"),
        "device.searching" => ("Поиск сканеров…", "Searching for scanners…"),
        "device.usb" => ("USB", "USB"),
        "device.network" => ("Сеть", "Network"),
        "device.unknown" => ("Тип неизвестен", "Unknown type"),
        "device.kind.usb" => ("USB-сканер", "USB scanner"),
        "device.kind.network" => ("Сетевой сканер", "Network scanner"),

        // --- Диалог добавления устройства ---
        "add_device.title" => ("Добавить устройство", "Add device"),
        "add_device.tab.auto" => ("Автопоиск", "Auto-detect"),
        "add_device.tab.ip" => ("По IP", "By IP"),
        "add_device.auto.hint" =>
            ("Нажмите «Обновить», чтобы найти USB и сетевые сканеры (SANE, eSCL/AirScan, WSD).",
             "Press “Refresh” to find USB and network scanners (SANE, eSCL/AirScan, WSD)."),
        "add_device.ip.address" => ("IP-адрес", "IP address"),
        "add_device.ip.name" => ("Имя устройства", "Device name"),
        "add_device.ip.protocol" => ("Протокол", "Protocol"),
        "add_device.ip.check" => ("Проверить подключение", "Test connection"),
        "add_device.ip.checking" => ("Проверка…", "Testing…"),
        "add_device.ip.reachable" => ("Устройство отвечает", "Device is reachable"),
        "add_device.ip.unreachable" => ("Устройство недоступно", "Device is unreachable"),
        "add_device.ip.config_hint" =>
            ("Для постоянной работы по IP добавьте устройство в /etc/sane.d/airscan.conf:",
             "For a permanent setup add the device to /etc/sane.d/airscan.conf:"),
        "add_device.ip.copy" => ("Скопировать", "Copy"),
        "add_device.ip.copied" => ("Скопировано", "Copied"),
        "add_device.ip.proto.auto" => ("Авто", "Auto"),
        "add_device.saved" => ("Устройство сохранено", "Device saved"),

        // --- Параметры сканирования ---
        "opt.profile" => ("Профиль", "Profile"),
        "opt.profile.default" => ("По умолчанию", "Default"),
        "opt.profile.save_current" => ("Сохранить текущий…", "Save current…"),
        "opt.source" => ("Источник", "Source"),
        "opt.source.flatbed" => ("Планшет", "Flatbed"),
        "opt.source.adf" => ("Податчик (ADF)", "ADF"),
        "opt.source.adf_duplex" => ("Податчик, дуплекс", "ADF Duplex"),
        "opt.resolution" => ("Разрешение, DPI", "Resolution, DPI"),
        "opt.mode" => ("Режим", "Mode"),
        "opt.mode.color" => ("Цвет", "Color"),
        "opt.mode.gray" => ("Оттенки серого", "Grayscale"),
        "opt.mode.lineart" => ("Чёрно-белый", "Black and white"),
        "opt.format" => ("Формат страницы", "Page format"),
        "opt.format.a4" => ("A4", "A4"),
        "opt.format.a5" => ("A5", "A5"),
        "opt.format.letter" => ("Letter", "Letter"),
        "opt.format.max" => ("Максимальный", "Maximum"),

        // --- Действия ---
        "action.scan" => ("Сканировать", "Scan"),
        "action.preview" => ("Предпросмотр", "Preview"),
        "action.cancel" => ("Прервать", "Abort"),
        "action.save_pdf" => ("Сохранить PDF", "Save PDF"),
        "action.export_images" => ("Экспорт изображений", "Export images"),
        "action.import_files" => ("Загрузить изображения", "Import images"),
        "action.rotate_left" => ("Повернуть влево", "Rotate left"),
        "action.rotate_right" => ("Повернуть вправо", "Rotate right"),
        "action.crop" => ("Обрезать", "Crop"),
        "action.enhance" => ("Автоконтраст", "Auto contrast"),
        "action.autofix" => ("Выправить и обрезать", "Straighten & crop"),
        "action.move_left" => ("Сдвинуть влево", "Move left"),
        "action.move_right" => ("Сдвинуть вправо", "Move right"),
        "action.more" => ("Ещё действия", "More actions"),
        "action.rotate_all_left" => ("Повернуть все влево", "Rotate all left"),
        "action.rotate_all_right" => ("Повернуть все вправо", "Rotate all right"),
        "action.enhance_all" => ("Автоконтраст всем страницам", "Auto contrast for all pages"),
        "action.autofix_all" => ("Выправить и обрезать все страницы", "Straighten & crop all pages"),
        "action.clear_all" => ("Удалить все страницы", "Remove all pages"),

        // --- Страницы / статус ---
        "pages.empty.title" => ("Пока нет страниц", "No pages yet"),
        "pages.empty.hint" =>
            ("Отсканируйте документ, загрузите изображения кнопкой ниже\nили просто перетащите файлы в окно.",
             "Scan a document, import images with the button below\nor just drag & drop files here."),
        "pages.count" => ("Страниц: {}", "Pages: {}"),
        "page.n" => ("Страница {}", "Page {}"),
        "dnd.added" => ("Добавлено страниц: {}", "Added {} pages"),
        "dnd.unsupported" => ("Поддерживаются только PNG, JPEG и TIFF", "Only PNG, JPEG and TIFF are supported"),
        "autofix.running" => ("Выправление страниц…", "Straightening pages…"),
        "autofix.done" => ("Выправлено страниц: {}", "Straightened {} pages"),
        "autofix.nothing" => ("Выберите страницу", "Select a page first"),
        "status.scanning" => ("Сканирование…", "Scanning…"),
        "status.page" => ("Страница {}", "Page {}"),
        "status.cancelling" => ("Остановка…", "Stopping…"),
        "status.scan_done" => ("Готово: страниц {}", "Done: {} pages"),
        "status.scan_cancelled" => ("Сканирование прервано", "Scan aborted"),
        "status.scan_failed" => ("Ошибка сканирования", "Scan failed"),
        "status.saved" => ("Сохранено: {}", "Saved: {}"),
        "status.export_failed" => ("Ошибка сохранения", "Export failed"),
        "status.no_device" => ("Сначала выберите сканер", "Select a scanner first"),
        "status.no_pages" => ("Нет страниц для сохранения", "No pages to save"),
        "toast.open_folder" => ("Открыть папку", "Open folder"),

        // --- Диалог обрезки ---
        "crop.title" => ("Обрезка страницы", "Crop page"),
        "crop.top" => ("Сверху, %", "Top, %"),
        "crop.bottom" => ("Снизу, %", "Bottom, %"),
        "crop.left" => ("Слева, %", "Left, %"),
        "crop.right" => ("Справа, %", "Right, %"),
        "crop.reset" => ("Сбросить", "Reset"),

        // --- Профили ---
        "profiles.title" => ("Профили сканирования", "Scan profiles"),
        "profiles.name" => ("Название профиля", "Profile name"),
        "profiles.save" => ("Сохранить профиль", "Save profile"),
        "profiles.delete" => ("Удалить", "Delete"),
        "profiles.empty" => ("Нет сохранённых профилей", "No saved profiles"),
        "profiles.saved" => ("Профиль сохранён", "Profile saved"),
        "profiles.applied" => ("Профиль применён", "Profile applied"),

        // --- Настройки / журнал ---
        "settings.menu" => ("Настройки", "Settings"),
        "settings.language" => ("Язык", "Language"),
        "settings.theme" => ("Тема", "Theme"),
        "settings.theme.system" => ("Системная", "System"),
        "settings.theme.light" => ("Светлая", "Light"),
        "settings.theme.dark" => ("Тёмная", "Dark"),
        "settings.log" => ("Журнал ошибок", "Error log"),
        "settings.about" => ("О программе", "About"),
        "log.empty" => ("Журнал пуст", "Log is empty"),
        "lang.changed" => ("Язык изменён. Перезапустите приложение.", "Language changed. Please restart the app."),
        "theme.changed" => ("Тема изменена", "Theme changed"),

        // --- О программе ---
        "about.comments" =>
            ("Универсальный фронтенд для SANE: USB, eSCL/AirScan, WSD, ADF и дуплекс.",
             "A universal SANE frontend: USB, eSCL/AirScan, WSD, ADF and duplex."),
        "about.website" => ("Веб-сайт", "Website"),

        // --- Состояние устройств (0.2) ---
        "device.status.ok" => ("Доступен", "Available"),
        "device.status.down" => ("Недоступен", "Unavailable"),
        "device.status.unknown" => ("Не проверялся", "Not checked"),
        "device.check_again" => ("Проверить снова", "Check again"),

        // --- Ручной дуплекс (0.2) ---
        "process.group" => ("Обработка", "Processing"),
        "duplex.row" => ("Ручной дуплекс", "Manual duplex"),
        "duplex.row.subtitle" =>
            ("Два прохода с переворотом стопа", "Two passes with a stack flip"),
        "duplex.flip.title" => ("Переверните стоп", "Flip the stack"),
        "duplex.flip.text" =>
            ("Лицевые стороны отсканированы ({} стр.). Переверните стоп и отсканируйте оборотные стороны.",
             "Front sides scanned ({} pages). Flip the stack and scan the back sides."),
        "duplex.flip.continue" => ("Сканировать оборотные", "Scan backs"),
        "duplex.done" => ("Дуплекс: собрано страниц {}", "Duplex: {} pages assembled"),
        "duplex.pass2" => ("Сканирование оборотных сторон…", "Scanning back sides…"),

        // --- Пустые страницы (0.2) ---
        "blank.row" => ("Пропускать пустые страницы", "Skip blank pages"),
        "blank.row.subtitle" =>
            ("Экономия размера PDF", "Saves PDF size"),
        "blank.skipped" => ("Пустая страница {} пропущена", "Blank page {} skipped"),

        // --- Предпросмотр устройства (0.2) ---
        "preview.title" => ("Предпросмотр сканера", "Scanner preview"),
        "preview.hint" =>
            ("Страница в низком разрешении — в документ не добавляется.",
             "Low-resolution page — it is not added to the document."),
        "preview.failed" => ("Не удалось получить предпросмотр", "Failed to get preview"),

        // --- Диалог экспорта (0.2) ---
        "export.title" => ("Экспорт документа", "Export document"),
        "export.format" => ("Формат", "Format"),
        "export.filename" => ("Имя файла", "File name"),
        "export.template.hint" =>
            ("Шаблон: {date} {time} {profile} {device}",
             "Template: {date} {time} {profile} {device}"),
        "export.ocr" => ("Распознать текст (OCR)", "Recognize text (OCR)"),
        "export.ocr.subtitle" =>
            ("Поисковый PDF через локальный Tesseract", "Searchable PDF via local Tesseract"),
        "export.ocr.unavailable" =>
            ("Tesseract не найден (пакеты tesseract, tesseract-rus)",
             "Tesseract not found (packages tesseract, tesseract-rus)"),
        "export.ocr.lang" => ("Язык распознавания", "OCR language"),
        "export.sign" => ("Подписать (ГОСТ)", "Sign (GOST)"),
        "export.sign.subtitle" =>
            ("Выполнить команду подписи после экспорта", "Run the signing command after export"),
        "export.run" => ("Экспортировать…", "Export…"),
        "export.to_folder" => ("Отправить в папку…", "Send to folder…"),
        "export.to_email" => ("Отправить по почте…", "Send by email…"),
        "export.copied" => ("Скопировано в папку: {}", "Copied to folder: {}"),
        "export.email.opened" => ("Почтовый клиент открыт", "Email client opened"),
        "export.email.unavailable" => ("xdg-email не найден", "xdg-email not found"),
        "export.signed" => ("Подписано: {}", "Signed: {}"),

        // --- Настройки (0.2) ---
        "settings.dialog.title" => ("Настройки", "Settings"),
        "settings.watched" => ("Наблюдаемая папка", "Watched folder"),
        "settings.watched.subtitle" =>
            ("Импортировать изображения, появляющиеся в папке (запись МФУ)",
             "Import images appearing in the folder (MFP direct save)"),
        "settings.watched.choose" => ("Выбрать папку…", "Choose folder…"),
        "settings.watched.none" => ("Папка не выбрана", "No folder selected"),
        "settings.template" => ("Шаблон имени файла", "File name template"),
        "settings.sign_cmd" => ("Команда подписи (ГОСТ)", "Signing command (GOST)"),
        "settings.sign_cmd.hint" => ("Плейсхолдеры: {file} {out}", "Placeholders: {file} {out}"),
        "settings.sign_after" => ("Подписывать после экспорта", "Sign after export"),
        "settings.threshold" => ("Порог пустоты, %", "Blank threshold, %"),
        "settings.l10n_folder" => ("Папка переводов", "Translations folder"),
        "settings.l10n.hint" =>
            ("Добавьте <код языка>.json — язык появится без перекомпиляции.",
             "Add <language-code>.json — the language appears without recompiling."),
        "watched.added" => ("Наблюдаемая папка: добавлено {}", "Watched folder: {} added"),

        // --- Обрезка (0.2) ---
        "crop.hint" =>
            ("Тяните углы рамки или перетаскивайте саму рамку",
             "Drag the corner handles or the frame itself"),

        _ => ("", ""),
    };
    let s = match lang {
        Lang::Ru => ru,
        Lang::En => en,
    };
    s.to_string()
}

/// Вспомогательный макрос для строк с подстановкой одного значения:
/// `tf!(lang, "pages.count", n)` -> "Страниц: 5".
#[macro_export]
macro_rules! tf {
    ($lang:expr, $key:expr, $arg:expr) => {
        $crate::i18n::t($lang, $key).replacen("{}", &$arg.to_string(), 1)
    };
}
