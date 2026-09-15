//! Экспорт: сборка PDF (lopdf) и PNG/JPEG/TIFF (image/tiff).

use std::path::{Path, PathBuf};

use crate::errors::{Result, ScannerError};

/// Имя файла по умолчанию: Scan_ГГГГ-ММ-ДД_ЧЧ-ММ-СС.ext
pub fn auto_filename(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    dir.join(format!("{base}_{ts}.{ext}"))
}

/// Сборка многостраничного PDF из изображений страниц.
///
/// Страницы встраиваются как JPEG (DCTDecode) — минимальный размер файла
/// без внешних зависимостей.
pub fn export_pdf(pages: &[PathBuf], out: &Path, dpi: u32, quality: u8) -> Result<()> {
    use lopdf::{dictionary, Document, Object, Stream};

    if pages.is_empty() {
        return Err(ScannerError::Other("нет страниц для экспорта".into()));
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => Object::Integer(0),
        "Kids" => Object::Array(Vec::new()),
    });

    for path in pages {
        let img = crate::imageproc::load(path)?;
        let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
        let (w_px, h_px) = (rgb.width() as f32, rgb.height() as f32);
        let w_pt = w_px * 72.0 / dpi as f32;
        let h_pt = h_px * 72.0 / dpi as f32;

        // Кодируем страницу в JPEG.
        use image::ImageEncoder;
        let mut jpeg: Vec<u8> = Vec::new();
        let pixels = rgb.to_rgb8();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        enc.write_image(pixels.as_raw(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgb8)
            .map_err(|e| ScannerError::Image(e.to_string()))?;

        let img_dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => Object::Integer(pixels.width() as i64),
            "Height" => Object::Integer(pixels.height() as i64),
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => Object::Integer(8),
            "Filter" => "DCTDecode",
        };
        let img_id = doc.add_object(Object::Stream(Stream::new(img_dict, jpeg)));

        // Content: отрисовать картинку на всю страницу.
        let content = format!("q {w_pt:.2} 0 0 {h_pt:.2} 0 0 cm /Im1 Do Q");
        let content_id = doc.add_object(Object::Stream(Stream::new(
            dictionary! {},
            content.into_bytes(),
        )));

        let page_dict = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(w_pt),
                Object::Real(h_pt),
            ],
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Im1" => img_id },
            },
            "Contents" => content_id,
        };
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        // Дописываем страницу в Kids и увеличиваем Count.
        let pages_obj = doc
            .get_object_mut(pages_id)
            .map_err(|e| ScannerError::Other(format!("lopdf: {e}")))?;
        if let Object::Dictionary(dict) = pages_obj {
            match dict.get_mut(b"Kids") {
                Ok(Object::Array(kids)) => kids.push(page_id.into()),
                _ => return Err(ScannerError::Other("lopdf: broken Kids".into())),
            }
            match dict.get_mut(b"Count") {
                Ok(Object::Integer(n)) => *n += 1,
                _ => return Err(ScannerError::Other("lopdf: broken Count".into())),
            }
        } else {
            return Err(ScannerError::Other("lopdf: broken Pages".into()));
        }
    }

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(out)
        .map_err(|e| ScannerError::Other(format!("lopdf save: {e}")))?;
    Ok(())
}

/// Экспорт PNG: одна страница -> один файл, несколько -> файлы с индексом.
pub fn export_png(pages: &[PathBuf], out_base: &Path) -> Result<Vec<PathBuf>> {
    export_images(pages, out_base, "png", |img, path| {
        img.save_with_format(path, image::ImageFormat::Png)
            .map_err(|e| ScannerError::Image(e.to_string()))
    })
}

/// Экспорт JPEG.
pub fn export_jpeg(pages: &[PathBuf], out_base: &Path, quality: u8) -> Result<Vec<PathBuf>> {
    export_images(pages, out_base, "jpg", move |img, path| {
        use image::ImageEncoder;
        let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8()).to_rgb8();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(path)?,
            quality,
        );
        enc.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
            .map_err(|e| ScannerError::Image(e.to_string()))
    })
}

/// Экспорт TIFF: одна страница -> один файл; несколько страниц ->
/// один МНОГОстраничный TIFF (каждый write_image начинает новую директорию
/// IFD, крейт сам выстраивает цепочку страниц).
pub fn export_tiff(pages: &[PathBuf], out_base: &Path) -> Result<Vec<PathBuf>> {
    if pages.is_empty() {
        return Err(ScannerError::Other("нет страниц для экспорта".into()));
    }
    use tiff::encoder::colortype::RGB8;
    use tiff::encoder::TiffEncoder;

    let path = out_base.with_extension("tiff");
    let file = std::fs::File::create(&path)?;
    let mut enc =
        TiffEncoder::new(file).map_err(|e| ScannerError::Image(format!("tiff: {e}")))?;

    for src in pages {
        let img = crate::imageproc::load(src)?;
        let rgb = img.to_rgb8();
        enc.write_image::<RGB8>(rgb.width(), rgb.height(), rgb.as_raw())
            .map_err(|e| ScannerError::Image(format!("tiff: {e}")))?;
    }
    Ok(vec![path])
}

fn export_images<F>(
    pages: &[PathBuf],
    out_base: &Path,
    ext: &str,
    writer: F,
) -> Result<Vec<PathBuf>>
where
    F: Fn(&image::DynamicImage, &Path) -> Result<()>,
{
    if pages.is_empty() {
        return Err(ScannerError::Other("нет страниц для экспорта".into()));
    }
    let mut written = Vec::new();
    for (i, src) in pages.iter().enumerate() {
        let img = crate::imageproc::load(src)?;
        let path = if pages.len() == 1 {
            out_base.with_extension(ext)
        } else {
            out_base.with_extension(format!("{:02}.{ext}", i + 1))
        };
        writer(&img, &path)?;
        written.push(path);
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Слияние готовых PDF (например, постраничных searchable-PDF от Tesseract)

/// Глубокое копирование объекта из исходного документа в целевой
/// с перенумерацией ссылок. Ключ `Parent` пропускается — дерево страниц
/// пересобирается заново. Скаляры (имена, числа, строки) копируются
/// как есть; рекурсируются только ссылки, словари, массивы и потоки.
fn deep_copy(
    src: &lopdf::Document,
    obj: &lopdf::Object,
    map: &mut std::collections::HashMap<(u32, u16), lopdf::ObjectId>,
    dst: &mut lopdf::Document,
) -> lopdf::Object {
    use lopdf::Object;
    if let Object::Reference(id) = obj {
        if let Some(existing) = map.get(id) {
            return Object::Reference(*existing);
        }
        if let Ok(resolved) = src.get_object(*id) {
            let copied = deep_copy(src, resolved, map, dst);
            let new_id = dst.add_object(copied);
            map.insert(*id, new_id);
            return Object::Reference(new_id);
        }
        // Битая ссылка — заглушка null.
        return Object::Null;
    }
    match obj {
        Object::Dictionary(dict) => {
            let mut out = lopdf::Dictionary::new();
            for (k, v) in dict.iter() {
                if k == b"Parent" {
                    continue;
                }
                out.set(k.to_vec(), deep_copy(src, v, map, dst));
            }
            Object::Dictionary(out)
        }
        Object::Array(items) => Object::Array(
            items.iter().map(|v| deep_copy(src, v, map, dst)).collect(),
        ),
        Object::Stream(stream) => {
            let mut dict = lopdf::Dictionary::new();
            for (k, v) in stream.dict.iter() {
                if k == b"Parent" {
                    continue;
                }
                dict.set(k.to_vec(), deep_copy(src, v, map, dst));
            }
            Object::Stream(lopdf::Stream::new(dict, stream.content.clone()))
        }
        other => other.clone(),
    }
}

/// Собрать один PDF из списка готовых PDF (каждый может быть многостраничным).
///
/// Используется для OCR: Tesseract умеет выдавать только постраничные
/// searchable-PDF, поэтому после распознавания страницы склеиваются здесь.
pub fn merge_pdfs(inputs: &[PathBuf], out: &Path) -> Result<()> {
    use lopdf::{dictionary, Object};

    if inputs.is_empty() {
        return Err(ScannerError::Other("нет PDF для слияния".into()));
    }

    let mut dest = lopdf::Document::with_version("1.5");
    let pages_id = dest.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => Object::Integer(0),
        "Kids" => Object::Array(Vec::new()),
    });

    for path in inputs {
        let src = lopdf::Document::load(path)
            .map_err(|e| ScannerError::Other(format!("lopdf load {}: {e}", path.display())))?;
        let mut map = std::collections::HashMap::new();
        // Ссылка на дерево страниц исходника: trailer -> Root -> /Pages.
        let src_pages_ref = (|| -> Result<lopdf::ObjectId> {
            let root = src
                .trailer
                .get(b"Root")
                .map_err(|e| ScannerError::Other(format!("lopdf: root: {e}")))?;
            let catalog = match root {
                Object::Reference(id) => src
                    .get_object(*id)
                    .map_err(|e| ScannerError::Other(format!("lopdf: catalog: {e}")))?,
                other => other,
            };
            match catalog
                .as_dict()
                .map_err(|e| ScannerError::Other(format!("lopdf: catalog dict: {e}")))?
                .get(b"Pages")
            {
                Ok(Object::Reference(id)) => Ok(*id),
                _ => Err(ScannerError::Other(format!(
                    "lopdf {}: не найдено дерево страниц",
                    path.display()
                ))),
            }
        })()?;
        let src_pages_obj = src
            .get_object(src_pages_ref)
            .map_err(|e| ScannerError::Other(format!("lopdf: {e}")))?;
        if let Object::Dictionary(d) = src_pages_obj {
            if let Ok(Object::Array(kids)) = d.get(b"Kids") {
                for kid in kids {
                    let copied = deep_copy(&src, kid, &mut map, &mut dest);
                    let new_ref = match copied {
                        Object::Reference(_) => copied,
                        other => Object::Reference(dest.add_object(other)),
                    };
                    // Дописать страницу в Kids целевого документа.
                    if let Object::Dictionary(dest_pages) = dest
                        .get_object_mut(pages_id)
                        .map_err(|e| ScannerError::Other(format!("lopdf: {e}")))?
                    {
                        if let Ok(Object::Array(arr)) = dest_pages.get_mut(b"Kids") {
                            arr.push(new_ref);
                        }
                        if let Ok(Object::Integer(n)) = dest_pages.get_mut(b"Count") {
                            *n += 1;
                        }
                    }
                }
            }
        }
    }

    let catalog_id = dest.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    dest.trailer.set("Root", Object::Reference(catalog_id));
    dest.save(out)
        .map_err(|e| ScannerError::Other(format!("lopdf save: {e}")))?;
    Ok(())
}
