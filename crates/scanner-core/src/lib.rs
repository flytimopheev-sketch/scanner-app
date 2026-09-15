//! scanner-core — ядро приложения сканирования документов.
//!
//! Слои (снизу вверх):
//! 1. Бэкенды SANE: `scanimage` (CLI), `sane_ffi` (libsane), `mock` (без железа)
//! 2. Задания и события: `scan_job`
//! 3. Хранилище: `store` (SQLite), конфигурация `config`
//! 4. Обработка: `imageproc`, экспорт `export`, OCR `ocr`, подписание `sign`
//! 5. Имена: `naming` (шаблоны), `duplex` (ручной дуплекс)
//! 6. Локализация: `i18n`

pub mod backend;
pub mod config;
pub mod device;
pub mod duplex;
pub mod errors;
pub mod export;
pub mod i18n;
pub mod imageproc;
pub mod mock;
pub mod naming;
pub mod ocr;
pub mod options;
pub mod sane_ffi;
pub mod scan_job;
pub mod scanimage;
pub mod sign;
pub mod store;

pub use backend::{create_backend, DeviceCapabilities, ScanProgress, ScannerBackend, TestResult};
pub use device::{DeviceKind, ScannerDevice};
pub use errors::{Result, ScannerError};
pub use options::{PageFormat, ScanMode, ScanOptions, ScanSource};
pub use scan_job::{
    run_scan_job, CapabilitiesResult, DeviceListResult, PreviewResult, Request, Response,
    ScanEvent, ScanResult, TestIpResult, WorkerRequest,
};
pub use store::{ScanProfile, Store};

#[cfg(test)]
mod tests {
    use crate::options::{ScanMode, ScanSource};
    use crate::scanimage::{parse_capabilities, parse_device_list};

    const SCANIMAGE_L: &str = "\
device `airscan:e0:Brother MFC-L2750DW' is a Brother MFC-L2750DW scanner
device `airscan:w1:HP LaserJet MFP' is a HP LaserJet MFP scanner
device `pixma:04A917XX_54E2F5' is a CANON Canon PIXMA MP610 multi-function peripheral
";

    const HELP: &str = "\
Options specific to device `airscan:e0:Brother MFC-L2750DW':
  Mode:
    --mode Color|Gray|Lineart [Color]
    --resolution 75..600dpi (in steps of 1) [300]
  Geometry:
    -l 0..215.9mm [0]
  Optional:
    --source Flatbed|ADF|ADF Duplex [ADF]
";

    #[test]
    fn parse_device_list_finds_three() {
        let devices = parse_device_list(SCANIMAGE_L);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].backend, "airscan");
        assert_eq!(devices[0].kind, crate::device::DeviceKind::Network);
        assert_eq!(devices[0].model.as_deref(), Some("Brother MFC-L2750DW"));
        assert_eq!(devices[2].backend, "pixma");
        assert_eq!(devices[2].name, "Canon PIXMA MP610");
    }

    #[test]
    fn parse_capabilities_full() {
        let caps = parse_capabilities(HELP);
        assert_eq!(caps.modes, vec![ScanMode::Color, ScanMode::Gray, ScanMode::Lineart]);
        assert!(caps.resolutions.contains(&75));
        assert!(caps.resolutions.contains(&600));
        assert_eq!(
            caps.sources,
            vec![ScanSource::Flatbed, ScanSource::Adf, ScanSource::AdfDuplex]
        );
    }

    #[test]
    fn ip_extraction() {
        assert_eq!(
            crate::device::ScannerDevice::from_sane_string(
                "airscan:escl:Test:https://192.168.1.50:8080",
                None,
                None
            )
            .address,
            Some("192.168.1.50".into())
        );
        assert_eq!(crate::device::ScannerDevice::from_sane_string("pixma:usb:04A9", None, None).kind, crate::device::DeviceKind::Usb);
    }

    #[test]
    fn manual_device_name_roundtrip() {
        let name = "manual|escl|192.168.1.50|8080|Office MFP";
        let parts = crate::scanimage::parse_manual(name).unwrap();
        assert_eq!(parts.0, "escl");
        assert_eq!(parts.1, "192.168.1.50");
        assert_eq!(parts.2, "8080");
        assert_eq!(parts.3, "Office MFP");
    }

    #[test]
    fn i18n_both_langs() {
        let ru = crate::i18n::t(crate::i18n::Lang::Ru, "action.scan");
        let en = crate::i18n::t(crate::i18n::Lang::En, "action.scan");
        assert_eq!(ru, "Сканировать");
        assert_eq!(en, "Scan");
    }

    #[test]
    fn imageproc_edit_pipeline() {
        let img = image::DynamicImage::new_rgb8(100, 200);
        let edited = crate::imageproc::apply_edit(
            img,
            &crate::imageproc::PageEdit {
                rotation: crate::imageproc::Rotation::Deg90,
                deskew: 0.0,
                crop: Some(crate::imageproc::CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.1,
                    bottom: 0.1,
                }),
                enhance: true,
            },
        );
        // После поворота 90°: 200x100, после обрезки 10% с каждой стороны: 160x80
        assert_eq!(edited.width(), 160);
        assert_eq!(edited.height(), 80);
    }

    /// Полосатое тестовое изображение: белые строки с чёрными «текстовыми»
    /// полосами, наклонёнными на заданный угол.
    fn skewed_document(angle_deg: f32, w: u32, h: u32) -> image::DynamicImage {
        let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));
        let rad = (angle_deg as f64).to_radians();
        let (sin, cos) = (rad.sin(), rad.cos());
        let cx = w as f64 / 2.0;
        let cy = h as f64 / 2.0;
        for y in 0..h {
            for x in 0..w {
                // Переводим в систему координат страницы (поворот на -angle).
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let sx = dx * cos - dy * sin;
                let sy = dx * sin + dy * cos;
                let u = sx + cx;
                let v = sy + cy;
                if u >= 0.0 && v >= 0.0 && u < w as f64 && v < h as f64 {
                    // Полосы текста: каждые 12 пикселей полоса 6 px.
                    if (v as u32 % 12) < 6 {
                        img.put_pixel(x, y, image::Luma([40]));
                    }
                }
            }
        }
        image::DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn detect_skew_finds_known_angle() {
        // Документ, наклонённый на 3°, должен распознаться с точностью 0.3°.
        let img = skewed_document(3.0, 240, 160);
        let angle = crate::imageproc::detect_skew(&img);
        assert!((angle - 3.0).abs() < 0.3, "got {angle}");
    }

    #[test]
    fn detect_skew_blank_page_is_zero() {
        // Пустая БЕЛАЯ страница: дисперсия проекции нулевая при любом угле.
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(300, 300, image::Luma([255])),
        );
        assert_eq!(crate::imageproc::detect_skew(&img), 0.0);
    }

    #[test]
    fn auto_crop_cuts_white_margins() {
        // Чёрный прямоугольник на белом поле 15% со всех сторон.
        let mut img = image::GrayImage::from_pixel(300, 400, image::Luma([255]));
        for y in 60..340 {
            for x in 45..255 {
                img.put_pixel(x, y, image::Luma([30]));
            }
        }
        let img = image::DynamicImage::ImageLuma8(img);
        let crop = crate::imageproc::auto_crop_rect(&img).expect("crop expected");
        // Поля ушли: слева/справа ~13-17%, сверху/снизу ~13-17%.
        assert!(crop.left > 0.10 && crop.left < 0.20, "left={}" , crop.left);
        assert!(crop.top > 0.10 && crop.top < 0.20, "top={}", crop.top);
    }

    #[test]
    fn auto_crop_full_page_is_none() {
        let mut img = image::GrayImage::from_pixel(300, 400, image::Luma([255]));
        for y in 2..398 {
            for x in 2..298 {
                img.put_pixel(x, y, image::Luma([30]));
            }
        }
        let img = image::DynamicImage::ImageLuma8(img);
        assert!(crate::imageproc::auto_crop_rect(&img).is_none());
    }

    #[test]
    fn rotate_fill_keeps_size() {
        let img = image::DynamicImage::new_rgb8(120, 80);
        let rot = crate::imageproc::rotate_fill(&img, 4.2, [255, 255, 255]);
        assert_eq!(rot.width(), 120);
        assert_eq!(rot.height(), 80);
    }

    #[test]
    fn export_tiff_multipage() {
        let dir = std::env::temp_dir().join(format!("scanner-core-tiff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = dir.join("p1.png");
        let p2 = dir.join("p2.png");
        image::DynamicImage::new_rgb8(60, 40).save_with_format(&p1, image::ImageFormat::Png).unwrap();
        image::DynamicImage::new_rgb8(60, 40).save_with_format(&p2, image::ImageFormat::Png).unwrap();
        let out = crate::export::export_tiff(&[p1, p2], &dir.join("out")).unwrap();
        assert_eq!(out.len(), 1, "один многостраничный файл");
        // Крейт tiff читает обе директории IFD.
        let file = std::fs::File::open(&out[0]).unwrap();
        let mut dec = tiff::decoder::Decoder::new(file).unwrap();
        // Первая страница уже загружена; дальше должна быть вторая.
        assert!(dec.more_images(), "вторая страница отсутствует");
        let _ = dec.next_image().unwrap();
        assert!(!dec.more_images(), "ожидались ровно две страницы");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_pdf_smoke() {
        let dir = std::env::temp_dir().join(format!("scanner-core-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("p1.png");
        image::DynamicImage::new_rgb8(120, 160)
            .save_with_format(&page, image::ImageFormat::Png)
            .unwrap();
        let out = dir.join("out.pdf");
        crate::export::export_pdf(&[page.clone()], &out, 300, 80).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.windows(4).any(|w| w == b"/Im1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_profiles_roundtrip() {
        let dir = std::env::temp_dir().join(format!("scanner-core-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = crate::store::Store::open(&db).unwrap();
        let profile = crate::store::ScanProfile {
            id: None,
            name: "Office".into(),
            device_name: Some("airscan:e0:X".into()),
            options: crate::options::ScanOptions::default(),
            duplex_manual: true,
            skip_blank: true,
            blank_threshold: 0.02,
        };
        let id = store.save_profile(&profile).unwrap();
        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, Some(id));
        assert_eq!(profiles[0].options.resolution, 300);
        assert!(profiles[0].duplex_manual);
        assert!(profiles[0].skip_blank);
        assert!((profiles[0].blank_threshold - 0.02).abs() < 1e-6);
        store.set_setting("lang", "en").unwrap();
        assert_eq!(store.get_setting("lang").as_deref(), Some("en"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_migration_from_old_schema() {
        // База версии 0.1: таблица profiles без новых колонок.
        let dir = std::env::temp_dir().join(format!("scanner-core-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("old.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE profiles (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    device_name TEXT,
                    resolution INTEGER NOT NULL DEFAULT 300,
                    mode TEXT NOT NULL DEFAULT 'color',
                    source TEXT NOT NULL DEFAULT 'flatbed',
                    format TEXT NOT NULL DEFAULT 'a4',
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 INSERT INTO profiles (name) VALUES ('Legacy');",
            )
            .unwrap();
        }
        let store = crate::store::Store::open(&db).unwrap();
        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert!(!profiles[0].duplex_manual, "старый профиль: дуплекс выключен");
        assert!(!profiles[0].skip_blank);
        assert!((profiles[0].blank_threshold - 0.01).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_detection() {
        // Чисто белая страница.
        let white = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(400, 300, image::Luma([255])),
        );
        assert!(crate::imageproc::is_blank(&white, 0.01));
        // Страница с «текстом» — 10% чернил.
        let mut text = image::GrayImage::from_pixel(400, 300, image::Luma([255]));
        for y in 0..300 {
            for x in 0..30 {
                text.put_pixel(x, y, image::Luma([30]));
            }
        }
        let text = image::DynamicImage::ImageLuma8(text);
        assert!(!crate::imageproc::is_blank(&text, 0.01));
        assert!(crate::imageproc::blank_ratio(&text, 200) > 0.05);
    }

    #[test]
    fn merge_pdfs_two_pages() {
        let dir = std::env::temp_dir().join(format!("scanner-core-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = dir.join("p1.png");
        image::DynamicImage::new_rgb8(80, 100)
            .save_with_format(&p1, image::ImageFormat::Png)
            .unwrap();
        let a = dir.join("a.pdf");
        let b = dir.join("b.pdf");
        crate::export::export_pdf(&[p1.clone()], &a, 150, 80).unwrap();
        crate::export::export_pdf(&[p1], &b, 150, 80).unwrap();
        let merged = dir.join("m.pdf");
        crate::export::merge_pdfs(&[a, b], &merged).unwrap();
        let doc = lopdf::Document::load(&merged).unwrap();
        assert_eq!(doc.get_pages().len(), 2, "после слияния две страницы");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_preview_produces_png() {
        use crate::backend::ScannerBackend;
        let backend = crate::mock::MockBackend::new();
        let dir = std::env::temp_dir().join(format!("scanner-core-pv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = backend.preview("mock:dev", 75, &dir).unwrap();
        assert!(path.exists(), "файл предпросмотра создан");
        let img = crate::imageproc::load(&path).unwrap();
        assert!(img.width() > 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
