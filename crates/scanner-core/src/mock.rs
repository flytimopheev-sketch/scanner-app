//! Mock-бэкенд: генерирует синтетические страницы вместо реального сканера.
//! Полезен для разработки и демонстрации интерфейса без железа.
//! Включается переменной окружения `SCANNER_BACKEND=mock`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{DeviceCapabilities, ScanProgress, ScannerBackend, TestResult};
use crate::device::{DeviceKind, ScannerDevice};
use crate::errors::{Result, ScannerError};
use crate::options::{ScanMode, ScanOptions, ScanSource};

pub struct MockBackend {
    cancel: Arc<AtomicBool>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self { cancel: Arc::new(AtomicBool::new(false)) }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Рисуем синтетическую «страницу документа».
fn render_page(idx: u32, mode: ScanMode, dpi: u32) -> image::DynamicImage {
    let w = (8_267u32 * dpi.min(300) / 600).max(200);
    let h = (11_693u32 * dpi.min(300) / 600).max(280);
    let mut img = image::RgbImage::new(w, h);
    let ink = match mode {
        ScanMode::Color => image::Rgb([235, 238, 244]),
        ScanMode::Gray => image::Rgb([236, 236, 236]),
        ScanMode::Lineart => image::Rgb([255, 255, 255]),
    };
    for px in img.pixels_mut() {
        *px = ink;
    }

    // Рамка и «текст» из тёмных полос (толщина 2+ пикселя, иначе линии
    // исчезают при уменьшении эскизов).
    let ink2 = match mode {
        ScanMode::Color => image::Rgb([70, 90, 130]),
        _ => image::Rgb([40, 40, 40]),
    };
    let margin = (w / 20) as i64;
    for t in 0..3 {
        let off = (t * 3) as i64;
        for x in margin..w as i64 - margin {
            for d in 0..2i64 {
                img.put_pixel(x as u32, (margin + off + d) as u32, ink2);
                img.put_pixel(x as u32, (h as i64 - margin - off - d - 1) as u32, ink2);
            }
        }
        for y in margin..h as i64 - margin {
            for d in 0..2i64 {
                img.put_pixel((margin + off + d) as u32, y as u32, ink2);
                img.put_pixel((w as i64 - margin - off - d - 1) as u32, y as u32, ink2);
            }
        }
    }
    // «Строки текста»
    let rows = (18 + (idx as usize * 3 % 8)) as u32;
    let thick = (dpi / 100).max(2) as i64;
    for r in 0..rows {
        let y = margin * 3 + r as i64 * (h as i64 - margin * 5) / rows.max(1) as i64;
        let width = if r % 5 == 0 { w / 2 } else { (w - margin as u32 * 2) * (60 + (r * 37) % 40) / 100 };
        for x in margin..margin + width as i64 {
            for dy in 0..thick {
                let yy = (y + dy).min(h as i64 - 1);
                img.put_pixel(x as u32, yy as u32, ink2);
            }
        }
    }
    image::DynamicImage::ImageRgb8(img)
}

impl ScannerBackend for MockBackend {
    fn list_devices(&self) -> Result<Vec<ScannerDevice>> {
        Ok(vec![
            ScannerDevice::from_sane_string(
                "airscan:e0:Brother MFC-L2750DW",
                Some("Brother".into()),
                Some("Brother MFC-L2750DW".into()),
            ),
            ScannerDevice::from_sane_string(
                "airscan:w1:HP LaserJet MFP M428",
                Some("HP".into()),
                Some("HP LaserJet MFP M428".into()),
            ),
            ScannerDevice::from_sane_string(
                "pixma:04A917XX_54E2F5",
                Some("Canon".into()),
                Some("Canon PIXMA MP610".into()),
            ),
        ])
    }

    fn capabilities(&self, _device: &str) -> Result<DeviceCapabilities> {
        Ok(DeviceCapabilities {
            resolutions: vec![75, 150, 200, 300, 600],
            modes: ScanMode::all().to_vec(),
            sources: vec![
                ScanSource::Flatbed,
                ScanSource::Adf,
                ScanSource::AdfDuplex,
            ],
        })
    }

    fn scan(
        &self,
        device: &str,
        options: &ScanOptions,
        out_dir: &Path,
        progress: &dyn Fn(ScanProgress),
    ) -> Result<Vec<PathBuf>> {
        options.validate()?;
        std::fs::create_dir_all(out_dir)?;
        let duplex = options.source == ScanSource::AdfDuplex;
        let pages = match options.source {
            ScanSource::Flatbed => 1,
            _ => 4,
        };
        let mut files = Vec::new();
        for i in 1..=pages {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(ScannerError::Cancelled);
            }
            progress(ScanProgress::PageStarted(i));
            // Имитация движения каретки/податчика
            for step in 1..=10 {
                if self.cancel.load(Ordering::SeqCst) {
                    return Err(ScannerError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(120));
                progress(ScanProgress::PageProgress(i, step as f32 * 10.0));
            }
            let img = render_page(i + duplex as u32 * 100, options.mode, options.resolution.min(300));
            let path = out_dir.join(format!("page-{i:04}.png"));
            img.save_with_format(&path, image::ImageFormat::Png)
                .map_err(|e| ScannerError::Image(e.to_string()))?;
            progress(ScanProgress::PageDone(i, path.clone()));
            files.push(path);
        }
        let _ = device;
        Ok(files)
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn test_ip(&self, address: &str, _protocol: &str) -> Result<TestResult> {
        Ok(TestResult {
            ok: true,
            device_name: None,
            message: format!("mock: {address} всегда доступен"),
        })
    }

    /// Предпросмотр: одна маленькая страница, без прогресса и задержек.
    fn preview(&self, _device: &str, dpi: u32, out_dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(out_dir)?;
        let img = render_page(0, ScanMode::Gray, dpi.clamp(50, 150));
        let path = out_dir.join("preview.png");
        img.save_with_format(&path, image::ImageFormat::Png)
            .map_err(|e| ScannerError::Image(e.to_string()))?;
        Ok(path)
    }
}

// Проверка kind-модели без железа.
#[allow(dead_code)]
fn kind_smoke() {
    let d = ScannerDevice::from_sane_string("airscan:e0:Test", None, None);
    debug_assert_eq!(d.kind, DeviceKind::Network);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autofix_on_mock_page_is_sane() {
        let img = render_page(1, ScanMode::Gray, 200);
        let w0 = img.width();
        let (angle, crop) = crate::imageproc::auto_fix(&img);
        println!("mock autofix: angle={angle}, crop={crop:?}");
        match crop {
            Some(c) => {
                assert!(c.left < 0.2 && c.top < 0.2, "crop too aggressive: {c:?}");
                assert!(c.right < 0.2 && c.bottom < 0.2, "crop too aggressive: {c:?}");
            }
            None => println!("crop: none"),
        }
        let edited = crate::imageproc::apply_edit(
            img,
            &crate::imageproc::PageEdit {
                rotation: crate::imageproc::Rotation::Deg0,
                deskew: angle,
                crop,
                enhance: false,
            },
        );
        println!("edited size: {}x{}", edited.width(), edited.height());
        assert!(edited.width() > w0 / 3, "page became tiny: {}", edited.width());
    }
}

#[cfg(test)]
mod disk_pipeline_tests {
    use super::*;

    /// Точная имитация GUI-пайплайна: страница на диске -> apply_edit -> thumb.
    #[test]
    fn thumb_after_crop_has_ink() {
        let dir = std::env::temp_dir().join(format!("gui-pipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("page-0001.png");
        render_page(1, ScanMode::Gray, 200)
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();

        let edit = crate::imageproc::PageEdit {
            rotation: crate::imageproc::Rotation::Deg0,
            deskew: 0.0,
            crop: Some(crate::imageproc::CropRect {
                left: 0.03539823, top: 0.020833334,
                right: 0.032448377, bottom: 0.020833334,
            }),
            enhance: false,
        };
        let img = crate::imageproc::load(&src).unwrap();
        let img = crate::imageproc::apply_edit(img, &edit);
        let thumb = crate::imageproc::thumbnail(&img, 360);
        let out = dir.join("thumb.png");
        crate::imageproc::save(&thumb, &out, 95).unwrap();
        let loaded = crate::imageproc::load(&out).unwrap();
        let luma = loaded.to_luma8();
        let mut mn = 255u8; let mut mx = 0u8;
        for p in luma.pixels() { mn = mn.min(p[0]); mx = mx.max(p[0]); }
        println!("thumb extrema: {mn}..{mx}");
        assert!(mn < 160, "thumb lost the ink: min={mn}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod compare_tests {
    use super::*;

    #[test]
    fn thumb_with_and_without_crop() {
        let dir = std::env::temp_dir().join(format!("gui-cmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("p.png");
        render_page(1, ScanMode::Gray, 200)
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();
        for (name, edit) in [
            ("nocrop", crate::imageproc::PageEdit::default()),
            ("crop", crate::imageproc::PageEdit {
                crop: Some(crate::imageproc::CropRect {
                    left: 0.0354, top: 0.0208, right: 0.0324, bottom: 0.0208 }),
                ..Default::default()
            }),
        ] {
            let img = crate::imageproc::load(&src).unwrap();
            let img = crate::imageproc::apply_edit(img, &edit);
            let thumb = crate::imageproc::thumbnail(&img, 360);
            let out = dir.join(format!("{name}.png"));
            crate::imageproc::save(&thumb, &out, 95).unwrap();
            let luma = crate::imageproc::load(&out).unwrap().to_luma8();
            let hist_bad = luma.pixels().filter(|p| p[0] < 120).count();
            println!("{name}: size={}x{} пикселей <120: {}", luma.width(), luma.height(), hist_bad);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
