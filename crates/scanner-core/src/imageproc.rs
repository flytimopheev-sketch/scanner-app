//! Обработка страниц: поворот, выправление, обрезка, авто-контраст, эскизы.
//!
//! Авто-выправление (deskew) — по методу проекционного профиля: ищем угол,
//! при котором дисперсия построчной гистограммы «чернил» максимальна.
//! Авто-обрезка — bounding box содержимого с полями и защитой от ложных
//! срабатываний (шум, чистый лист, киросший край скана).

use std::path::Path;

use crate::errors::{Result, ScannerError};

/// Поворот страницы по часовой стрелке (в градусах, кратные 90).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    pub fn next_cw(self) -> Self {
        match self {
            Rotation::Deg0 => Rotation::Deg90,
            Rotation::Deg90 => Rotation::Deg180,
            Rotation::Deg180 => Rotation::Deg270,
            Rotation::Deg270 => Rotation::Deg0,
        }
    }

    pub fn next_ccw(self) -> Self {
        self.next_cw().next_cw().next_cw()
    }
}

impl Default for Rotation {
    fn default() -> Self {
        Rotation::Deg0
    }
}

/// Прямоугольник обрезки в долях страницы (0.0..1.0).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        CropRect { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }
    }
}

impl CropRect {
    pub fn is_empty(&self) -> bool {
        self.left <= 0.001 && self.top <= 0.001 && self.right <= 0.001 && self.bottom <= 0.001
    }
}

/// Правки конкретной страницы.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageEdit {
    pub rotation: Rotation,
    /// Доворот на произвольный угол в градусах (-15..15), выправление перекоса.
    #[serde(default)]
    pub deskew: f32,
    pub crop: Option<CropRect>,
    /// Автоконтраст (растяжка гистограммы) — полезно для фото документов.
    pub enhance: bool,
}

impl PageEdit {
    pub fn is_identity(&self) -> bool {
        self.rotation == Rotation::Deg0
            && self.deskew.abs() < 0.01
            && self.crop.map(|c| c.is_empty()).unwrap_or(true)
            && !self.enhance
    }

    /// Числовой ключ поворота (для кэшей).
    pub fn rotation_key(&self) -> u8 {
        match self.rotation {
            Rotation::Deg0 => 0,
            Rotation::Deg90 => 1,
            Rotation::Deg180 => 2,
            Rotation::Deg270 => 3,
        }
    }
}

/// Загрузка изображения по расширению (tiff/tif, png, jpg/jpeg).
pub fn load(path: &Path) -> Result<image::DynamicImage> {
    image::open(path).map_err(|e| ScannerError::Image(format!("{}: {e}", path.display())))
}

/// Применить правки к изображению страницы.
pub fn apply_edit(img: image::DynamicImage, edit: &PageEdit) -> image::DynamicImage {
    let mut img = match edit.rotation {
        Rotation::Deg0 => img,
        Rotation::Deg90 => img.rotate90(),
        Rotation::Deg180 => img.rotate180(),
        Rotation::Deg270 => img.rotate270(),
    };
    if edit.deskew.abs() >= 0.05 {
        img = rotate_fill(&img, edit.deskew, [255, 255, 255]);
    }
    if let Some(crop) = edit.crop {
        if !crop.is_empty() {
            img = apply_crop(img, &crop);
        }
    }
    if edit.enhance {
        img = auto_contrast(&img);
    }
    img
}

/// Поворот на произвольный угол вокруг центра, размер кадра сохраняется,
/// углы заполняются цветом фона. Инверсное отображение + билинейная выборка.
pub fn rotate_fill(img: &image::DynamicImage, deg: f32, fill: [u8; 3]) -> image::DynamicImage {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as i64, rgb.height() as i64);
    if w == 0 || h == 0 {
        return image::DynamicImage::ImageRgb8(rgb);
    }
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let rad = (deg as f64).to_radians();
    let (sin, cos) = rad.sin_cos();
    let mut out = image::RgbImage::from_pixel(rgb.width(), rgb.height(), image::Rgb(fill));
    let (last_x, last_y) = (w - 1, h - 1);
    for oy in 0..h {
        for ox in 0..w {
            let dx = ox as f64 - cx;
            let dy = oy as f64 - cy;
            // Обратное преобразование: координаты в исходном изображении.
            let sx = cx + dx * cos + dy * sin;
            let sy = cy - dx * sin + dy * cos;
            if sx >= 0.0 && sy >= 0.0 && sx < last_x as f64 && sy < last_y as f64 {
                let x0 = sx as i64;
                let y0 = sy as i64;
                let fx = sx - x0 as f64;
                let fy = sy - y0 as f64;
                let p = bilinear(&rgb, x0, y0, fx, fy);
                out.put_pixel(ox as u32, oy as u32, p);
            }
        }
    }
    image::DynamicImage::ImageRgb8(out)
}

fn bilinear(
    img: &image::RgbImage,
    x0: i64,
    y0: i64,
    fx: f64,
    fy: f64,
) -> image::Rgb<u8> {
    let g = |x: i64, y: i64| {
        let p = img.get_pixel(x as u32, y as u32);
        [p[0] as f64, p[1] as f64, p[2] as f64]
    };
    let (x1, y1) = (x0 + 1, y0 + 1);
    let (p00, p10, p01, p11) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let mut out = [0u8; 3];
    for c in 0..3 {
        let v = lerp(lerp(p00[c], p10[c], fx), lerp(p01[c], p11[c], fx), fy);
        out[c] = v.clamp(0.0, 255.0).round() as u8;
    }
    image::Rgb(out)
}

/// Оценка перекоса страницы в градусах (метод проекционного профиля).
/// Работает по уменьшенной полутоновой копии; возвращает угол в -8..8.
/// Для чистого/шумного кадра возвращает 0.
pub fn detect_skew(img: &image::DynamicImage) -> f32 {
    const MAX_SIDE: u32 = 480;
    let small = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let gray = small.to_luma8();
    if gray.width() < 8 || gray.height() < 8 {
        return 0.0;
    }
    // Грубый проход с шагом 0.5°, затем уточнение с шагом 0.1°.
    let mut best = (0.0f32, f64::MIN);
    let mut a = -8.0f32;
    while a <= 8.0 {
        let s = skew_score(&gray, a);
        if s > best.1 {
            best = (a, s);
        }
        a += 0.5;
    }
    let mut a = best.0 - 0.5;
    let hi = best.0 + 0.5;
    while a <= hi {
        let s = skew_score(&gray, a);
        if s > best.1 {
            best = (a, s);
        }
        a += 0.1;
    }
    let (angle, score) = best;
    // На пустых страницах все углы дают одинаковый (нулевой) счёт.
    if score <= f64::EPSILON {
        0.0
    } else {
        angle.clamp(-8.0, 8.0)
    }
}

/// Счёт угла: дисперсия построчных количеств «чернил» повёрнутого кадра.
fn skew_score(gray: &image::GrayImage, deg: f32) -> f64 {
    let rot = rotate_gray_fill(gray, deg, 255);
    let w = rot.width() as usize;
    let h = rot.height() as usize;
    let mut rows = vec![0u64; h];
    for (y, row) in rows.iter_mut().enumerate() {
        let mut acc = 0u64;
        for x in 0..w {
            if rot.get_pixel(x as u32, y as u32)[0] < 170 {
                acc += 1;
            }
        }
        *row = acc;
    }
    let n = rows.len() as f64;
    let mean: f64 = rows.iter().map(|&c| c as f64).sum::<f64>() / n;
    let var: f64 = rows.iter().map(|&c| {
        let d = c as f64 - mean;
        d * d
    }).sum::<f64>() / n;
    var
}

fn rotate_gray_fill(img: &image::GrayImage, deg: f32, fill: u8) -> image::GrayImage {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if w == 0 || h == 0 {
        return img.clone();
    }
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let rad = (deg as f64).to_radians();
    let (sin, cos) = rad.sin_cos();
    let mut out = image::GrayImage::from_pixel(img.width(), img.height(), image::Luma([fill]));
    let (last_x, last_y) = (w - 1, h - 1);
    for oy in 0..h {
        for ox in 0..w {
            let dx = ox as f64 - cx;
            let dy = oy as f64 - cy;
            let sx = cx + dx * cos + dy * sin;
            let sy = cy - dx * sin + dy * cos;
            if sx >= 0.0 && sy >= 0.0 && sx < last_x as f64 && sy < last_y as f64 {
                let x0 = sx as i64;
                let y0 = sy as i64;
                let fx = (sx - x0 as f64) as f32;
                let fy = (sy - y0 as f64) as f32;
                let p00 = img.get_pixel(x0 as u32, y0 as u32)[0] as f32;
                let p10 = img.get_pixel((x0 + 1) as u32, y0 as u32)[0] as f32;
                let p01 = img.get_pixel(x0 as u32, (y0 + 1) as u32)[0] as f32;
                let p11 = img.get_pixel((x0 + 1) as u32, (y0 + 1) as u32)[0] as f32;
                let top = p00 + (p10 - p00) * fx;
                let bot = p01 + (p11 - p01) * fx;
                let v = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
                out.put_pixel(ox as u32, oy as u32, image::Luma([v]));
            }
        }
    }
    out
}

/// Авто-обрезка: bounding box содержимого с полями.
/// Возвращает None, если режется мало (< 3% площади) или замер подозрителен.
/// Измерение ведётся по уменьшенной копии — доли обрезки не зависят от DPI.
pub fn auto_crop_rect(img: &image::DynamicImage) -> Option<CropRect> {
    const MAX_SIDE: u32 = 1000;
    let small = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let (w, h) = (small.width() as usize, small.height() as usize);
    if w < 32 || h < 32 {
        return None;
    }
    let gray = small.to_luma8();
    let mut row_ink = vec![0u32; h];
    let mut col_ink = vec![0u32; w];
    for y in 0..h {
        for x in 0..w {
            if gray.get_pixel(x as u32, y as u32)[0] < 200 {
                row_ink[y] += 1;
                col_ink[x] += 1;
            }
        }
    }
    // Строка/столбец значимы, если в них заметно больше чернил, чем шум.
    let row_thr = (w as u32 / 400).max(1);
    let col_thr = (h as u32 / 400).max(1);
    let top = row_ink.iter().position(|&c| c >= row_thr)?;
    let bottom = row_ink.iter().rposition(|&c| c >= row_thr)?;
    let left = col_ink.iter().position(|&c| c >= col_thr)?;
    let right = col_ink.iter().rposition(|&c| c >= col_thr)?;

    // Поля 1.5% стороны.
    let my = ((h as f32) * 0.015).round() as usize;
    let mx = ((w as f32) * 0.015).round() as usize;
    let top = top.saturating_sub(my);
    let left = left.saturating_sub(mx);
    let bottom = (bottom + my).min(h - 1);
    let right = (right + mx).min(w - 1);

    let cw = right.saturating_sub(left);
    let ch = bottom.saturating_sub(top);
    if cw == 0 || ch == 0 {
        return None;
    }
    // Экономия площади мизерная — обрезка не нужна.
    if (cw as f64 * ch as f64) / ((w * h) as f64) > 0.97 {
        return None;
    }
    // Защита от ошибки: одна сторона срезана больше чем на 45%.
    let cut_right = w - 1 - right;
    let cut_bottom = h - 1 - bottom;
    if left > w * 45 / 100
        || cut_right > w * 45 / 100
        || top > h * 45 / 100
        || cut_bottom > h * 45 / 100
    {
        return None;
    }
    Some(CropRect {
        left: left as f32 / w as f32,
        top: top as f32 / h as f32,
        right: (w - 1 - right) as f32 / w as f32,
        bottom: (h - 1 - bottom) as f32 / h as f32,
    })
}

/// Полный авто-фикс страницы: угол выправления + рамка обрезки.
/// База — изображение с уже применённым поворотом на 90°.
/// Все измерения — на копии длиной стороны до 480 пикселей: доля секунды
/// даже на сканах 600 DPI; финальный рендер всё равно делает apply_edit.
pub fn auto_fix(img: &image::DynamicImage) -> (f32, Option<CropRect>) {
    const MAX_SIDE: u32 = 480;
    let small = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let angle = detect_skew(&small);
    let deskewed;
    let target = if angle.abs() >= 0.05 {
        deskewed = rotate_fill(&small, angle, [255, 255, 255]);
        &deskewed
    } else {
        &small
    };
    (angle, auto_crop_rect(target))
}

/// Обрезка в долях сторон.
pub fn apply_crop(img: image::DynamicImage, crop: &CropRect) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let x = ((w as f32 * crop.left).round() as u32).min(w.saturating_sub(1));
    let y = ((h as f32 * crop.top).round() as u32).min(h.saturating_sub(1));
    let cw = ((w as f32 * (1.0 - crop.left - crop.right)).round() as u32)
        .min(w - x)
        .max(1);
    let ch = ((h as f32 * (1.0 - crop.top - crop.bottom)).round() as u32)
        .min(h - y)
        .max(1);
    img.crop_imm(x, y, cw, ch)
}

/// Автоконтраст: растяжка яркостной гистограммы по перцентилям 1%..99%.
pub fn auto_contrast(img: &image::DynamicImage) -> image::DynamicImage {
    let rgb = img.to_rgb8();
    let mut hist = [0u32; 256];
    for p in rgb.pixels() {
        let luma = ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as usize;
        hist[luma.min(255)] += 1;
    }
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return image::DynamicImage::ImageRgb8(rgb);
    }
    let lo_cut = total / 100;
    let hi_cut = total - lo_cut;
    let mut acc = 0u32;
    let mut lo = 0u8;
    let mut hi = 255u8;
    for (v, c) in hist.iter().enumerate() {
        acc += c;
        if acc >= lo_cut.max(1) {
            lo = v as u8;
            break;
        }
    }
    acc = 0;
    for (v, c) in hist.iter().enumerate().rev() {
        acc += c;
        if acc >= total - hi_cut {
            hi = v as u8;
            break;
        }
    }
    if hi <= lo + 2 {
        return image::DynamicImage::ImageRgb8(rgb);
    }
    // Таблица пересчёта
    let mut lut = [0u8; 256];
    for (v, item) in lut.iter_mut().enumerate() {
        let x = (v as f32 - lo as f32) * 255.0 / (hi - lo) as f32;
        *item = x.clamp(0.0, 255.0).round() as u8;
    }
    let mut out = rgb;
    for p in out.pixels_mut() {
        p[0] = lut[p[0] as usize];
        p[1] = lut[p[1] as usize];
        p[2] = lut[p[2] as usize];
    }
    image::DynamicImage::ImageRgb8(out)
}

/// Эскиз для галереи (длинная сторона = size).
pub fn thumbnail(img: &image::DynamicImage, size: u32) -> image::DynamicImage {
    img.resize(size, size, image::imageops::FilterType::Triangle)
}

/// Уменьшение до длинной стороны max_side (если крупнее).
pub fn downscale(img: image::DynamicImage, max_side: u32) -> image::DynamicImage {
    if img.width().max(img.height()) > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    }
}

/// Сохранение изображения в файл по расширению.
pub fn save(img: &image::DynamicImage, path: &Path, quality: u8) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let fmt = match ext.as_str() {
        "png" => image::ImageFormat::Png,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "tif" | "tiff" => image::ImageFormat::Tiff,
        other => {
            return Err(ScannerError::Image(format!("unknown extension: {other}")));
        }
    };
    let mut img = img.clone();
    if fmt == image::ImageFormat::Jpeg {
        img = image::DynamicImage::ImageRgb8(img.to_rgb8());
    }
    if fmt == image::ImageFormat::Jpeg {
        use image::ImageEncoder;
        let rgb = img.to_rgb8();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(path)?,
            quality,
        );
        enc.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| ScannerError::Image(e.to_string()))?;
        Ok(())
    } else {
        img.save_with_format(path, fmt)
            .map_err(|e| ScannerError::Image(e.to_string()))
    }
}

/// Слияние кадров трёхпроходного сканера (RED/GREEN/BLUE) — базовая реализация.
pub fn merge_frames(_prev: image::DynamicImage, frame: image::DynamicImage) -> image::DynamicImage {
    // Для MVP просто возвращаем последний кадр; полноценное слияние каналов — в roadmap.
    frame
}

// ---------------------------------------------------------------------------
// Пустые страницы (автопропуск для экономии размера PDF)

/// Доля «чернил» на странице: отношение пикселей темнее `ink_level`
/// (0..255) к общему числу. Измерение идёт на уменьшенной копии
/// (макс. сторона 400 px) — доля секунды даже для 600 DPI.
pub fn blank_ratio(img: &image::DynamicImage, ink_level: u8) -> f32 {
    const MAX_SIDE: u32 = 400;
    let small = downscale(img.clone(), MAX_SIDE).to_luma8();
    let total = small.width() as usize * small.height() as usize;
    if total == 0 {
        return 0.0;
    }
    let ink = small.pixels().filter(|p| p[0] < ink_level).count();
    ink as f32 / total as f32
}

/// Пустая ли страница: доля чернил не превышает порог `threshold`
/// (например, 0.01 = 1 % площади). Порог 0.05..0.2 отсекает и почти
/// пустые страницы с печатью/одной строкой.
pub fn is_blank(img: &image::DynamicImage, threshold: f32) -> bool {
    // 200 отсекает серый фон и лёгкий шум сканера, но не пропускает текст.
    blank_ratio(img, 200) <= threshold.max(0.0)
}

/// Быстрая проверка страницы на диске: пустая ли она.
pub fn is_blank_file(path: &Path, threshold: f32) -> Result<bool> {
    Ok(is_blank(&load(path)?, threshold))
}
