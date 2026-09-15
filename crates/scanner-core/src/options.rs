//! Параметры сканирования: разрешение, режим, источник, формат страницы.

use serde::{Deserialize, Serialize};

/// Цветовой режим.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    Color,
    Gray,
    Lineart,
}

impl ScanMode {
    /// Имя режима, как его ждёт `scanimage --mode=...`.
    pub fn scanimage_name(self) -> &'static str {
        match self {
            ScanMode::Color => "Color",
            ScanMode::Gray => "Gray",
            ScanMode::Lineart => "Lineart",
        }
    }

    /// Ключ для БД / i18n.
    pub fn key(self) -> &'static str {
        match self {
            ScanMode::Color => "color",
            ScanMode::Gray => "gray",
            ScanMode::Lineart => "lineart",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "color" => Some(ScanMode::Color),
            "gray" => Some(ScanMode::Gray),
            "lineart" => Some(ScanMode::Lineart),
            _ => None,
        }
    }

    pub fn all() -> [ScanMode; 3] {
        [ScanMode::Color, ScanMode::Gray, ScanMode::Lineart]
    }
}

/// Источник изображения (планшет / автоподатчик / дуплекс).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanSource {
    Flatbed,
    Adf,
    AdfDuplex,
}

impl ScanSource {
    /// Значение для `scanimage --source=...`. `None` — планшет по умолчанию.
    pub fn scanimage_name(self) -> Option<&'static str> {
        match self {
            ScanSource::Flatbed => None,
            ScanSource::Adf => Some("ADF"),
            ScanSource::AdfDuplex => Some("ADF Duplex"),
        }
    }

    /// Обратное сопоставление: строка SANE -> источник.
    pub fn from_scanimage_name(s: &str) -> Option<Self> {
        let s = s.trim();
        // Варианты встречаются: "Flatbed", "ADF", "ADF Duplex", "Automatic Document Feeder"
        if s.eq_ignore_ascii_case("adf duplex") || s.contains("Duplex") {
            Some(ScanSource::AdfDuplex)
        } else if s.eq_ignore_ascii_case("adf")
            || s.contains("ADF")
            || s.contains("Feeder")
            || s.contains("feeder")
        {
            Some(ScanSource::Adf)
        } else {
            // Flatbed / Auto / что-то неизвестное => считаем планшетом
            Some(ScanSource::Flatbed)
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ScanSource::Flatbed => "flatbed",
            ScanSource::Adf => "adf",
            ScanSource::AdfDuplex => "adf_duplex",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "flatbed" => Some(ScanSource::Flatbed),
            "adf" => Some(ScanSource::Adf),
            "adf_duplex" => Some(ScanSource::AdfDuplex),
            _ => None,
        }
    }

    pub fn all() -> [ScanSource; 3] {
        [ScanSource::Flatbed, ScanSource::Adf, ScanSource::AdfDuplex]
    }
}

/// Формат страницы.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageFormat {
    A4,
    A5,
    Letter,
    /// Максимальная область устройства (задаётся возможностями сканера).
    Max,
}

impl PageFormat {
    /// Размер в миллиметрах (ширина, высота).
    pub fn size_mm(self) -> (f32, f32) {
        match self {
            PageFormat::A4 => (210.0, 297.0),
            PageFormat::A5 => (148.0, 210.0),
            PageFormat::Letter => (215.9, 279.4),
            PageFormat::Max => (216.0, 297.0), // практический максимум планшета
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            PageFormat::A4 => "a4",
            PageFormat::A5 => "a5",
            PageFormat::Letter => "letter",
            PageFormat::Max => "max",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "a4" => Some(PageFormat::A4),
            "a5" => Some(PageFormat::A5),
            "letter" => Some(PageFormat::Letter),
            "max" => Some(PageFormat::Max),
            _ => None,
        }
    }

    pub fn all() -> [PageFormat; 4] {
        [PageFormat::A4, PageFormat::A5, PageFormat::Letter, PageFormat::Max]
    }
}

/// Полный набор параметров сканирования.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanOptions {
    /// 75..600 DPI
    pub resolution: u32,
    pub mode: ScanMode,
    pub source: ScanSource,
    pub format: PageFormat,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            resolution: 300,
            mode: ScanMode::Color,
            source: ScanSource::Flatbed,
            format: PageFormat::A4,
        }
    }
}

impl ScanOptions {
    /// Валидация значений перед запуском задания.
    pub fn validate(&self) -> crate::errors::Result<()> {
        if !(75..=600).contains(&self.resolution) {
            return Err(crate::errors::ScannerError::Other(format!(
                "resolution {} DPI вне диапазона 75..600",
                self.resolution
            )));
        }
        Ok(())
    }
}
