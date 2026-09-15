//! OCR через локальный Tesseract — для изолированных сетей РЕД ОС.
//!
//! Внешних Rust-зависимостей нет: модуль запускает системный бинарник
//! `tesseract` (пакеты tesseract + tesseract-rus / tesseract-eng).
//! Если Tesseract не установлен — функции возвращают
//! [`ScannerError::Unsupported`], а интерфейс просто скрывает OCR.
//!
//! Возможности:
//! * проверка доступности и списка языков (`tesseract --list-langs`);
//! * постраничное распознавание: изображение -> PDF с невидимым
//!   текстовым слоем (searchable PDF) и/или plain text;
//! * сборка итогового PDF: слияние постраничных PDF (`export::merge_pdfs`).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::{Result, ScannerError};

/// Название бинарника Tesseract.
pub const TESSERACT_BIN: &str = "tesseract";

/// Версия Tesseract (строка вида `tesseract 5.x.x`) либо None.
pub fn detect() -> Option<String> {
    let out = Command::new(TESSERACT_BIN).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("").trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Доступен ли Tesseract.
pub fn available() -> bool {
    detect().is_some()
}

/// Установленные языки распознавания (`--list-langs`).
pub fn list_langs() -> Vec<String> {
    let Ok(out) = Command::new(TESSERACT_BIN).arg("--list-langs").output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && *l != "List of available languages (1):")
        .map(|l| l.to_string())
        .collect()
}

/// Построить аргументы вызова tesseract для одной страницы.
/// Отдельная функция — для юнит-тестов.
pub fn build_page_args(
    image: &Path,
    out_base: &Path,
    langs: &str,
    want_pdf: bool,
    want_txt: bool,
) -> Vec<String> {
    let mut args = vec![
        image.to_string_lossy().into_owned(),
        out_base.to_string_lossy().into_owned(),
        "-l".to_string(),
        langs.to_string(),
    ];
    let mut outs = Vec::new();
    if want_txt {
        outs.push("txt");
    }
    if want_pdf {
        outs.push("pdf");
    }
    if !outs.is_empty() {
        args.push(outs.join("+"));
    }
    args
}

/// Распознать одну страницу.
///
/// * `image` — PNG/JPEG/TIFF со страницей;
/// * `out_base` — база выходных имён (без расширения);
/// * `langs` — строка языков tesseract, например `rus`, `eng`, `rus+eng`;
/// * `want_pdf` — создать постраничный searchable PDF (`out_base.pdf`);
/// * `want_txt` — создать текст (`out_base.txt`).
///
/// Возвращает (pdf_path, txt_path) — те, что были запрошены.
pub fn ocr_page(
    image: &Path,
    out_base: &Path,
    langs: &str,
    want_pdf: bool,
    want_txt: bool,
) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    if !want_pdf && !want_txt {
        return Err(ScannerError::Other("OCR: не выбран формат вывода".into()));
    }
    let args = build_page_args(image, out_base, langs, want_pdf, want_txt);
    let out = Command::new(TESSERACT_BIN)
        .args(&args)
        .output()
        .map_err(|e| {
            ScannerError::Unsupported(format!(
                "не удалось запустить {TESSERACT_BIN}: {e}"
            ))
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(ScannerError::Other(format!("tesseract: {}", err.trim())));
    }
    let pdf = if want_pdf {
        let p = out_base.with_extension("pdf");
        if !p.exists() {
            return Err(ScannerError::Other(format!(
                "tesseract не создал {}",
                p.display()
            )));
        }
        Some(p)
    } else {
        None
    };
    let txt = if want_txt {
        let p = out_base.with_extension("txt");
        Some(p)
    } else {
        None
    };
    Ok((pdf, txt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_include_langs_and_outputs() {
        let args = build_page_args(Path::new("/x/p1.png"), Path::new("/x/out"), "rus+eng", true, true);
        assert_eq!(
            args,
            vec!["/x/p1.png", "/x/out", "-l", "rus+eng", "txt+pdf"]
        );
        let args = build_page_args(Path::new("/x/p1.png"), Path::new("/x/out"), "rus", false, true);
        assert_eq!(args.last().map(|s| s.as_str()), Some("txt"));
    }

    #[test]
    fn ocr_page_without_output_kind_errors() {
        let err = ocr_page(Path::new("/nope.png"), Path::new("/nope"), "rus", false, false);
        assert!(err.is_err());
    }
}
