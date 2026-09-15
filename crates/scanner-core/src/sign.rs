//! Подписание PDF внешним криптоинструментом (контур РЕД ОС / ГОСТ).
//!
//! Полноценная ГОСТ-подпись требует национального криптопровайдера
//! (КриптоПро CSP, VipNet CSP), которые ставятся в замкнутом контуре
//! отдельно. Вместо дублирования криптографии приложение выполняет
//! настраиваемую команду-шаблон после экспорта:
//!
//! ```text
//! cryptcp -signf -detach -cert %CERT% {file}
//! openssl ... {file} {out}
//! # любая утилита контура
//! ```
//!
//! Подстановки: `{file}` — путь к PDF; `{out}` — путь результата
//! (файл с суффиксом `-signed`). Если плейсхолдер не указан, путь к
//! файлу добавляется в конец команды. Код возврата != 0 считается
//! ошибкой подписания; stderr попадает в журнал ошибок.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::{Result, ScannerError};

/// Результат подписания.
#[derive(Debug, Clone)]
pub struct SignOutcome {
    /// Созданный подписанный файл (если команда его создала).
    pub output: Option<PathBuf>,
    /// Полный текст выполненной команды (для журнала).
    pub command: String,
}

/// Подставить файл в шаблон команды.
/// Отдельная функция — для юнит-тестов.
pub fn build_command(tpl: &str, file: &Path, out: &Path) -> String {
    if tpl.contains("{file}") || tpl.contains("{out}") {
        tpl.replace("{file}", &file.to_string_lossy())
            .replace("{out}", &out.to_string_lossy())
    } else {
        format!("{tpl} {}", file.display())
    }
}

/// Подписать PDF командой из шаблона.
///
/// Выполнение идёт через `sh -c`, поэтому допустимы конвейеры и
/// переменные окружения. Успех = код возврата 0.
pub fn sign_pdf(tpl: &str, file: &Path) -> Result<SignOutcome> {
    let tpl = tpl.trim();
    if tpl.is_empty() {
        return Err(ScannerError::Other("команда подписи не задана".into()));
    }
    let stem = file.with_extension("");
    let out = std::path::PathBuf::from(format!("{}-signed.pdf", stem.display()));
    let cmd = build_command(tpl, file, &out);
    let res = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .map_err(|e| ScannerError::Other(format!("подписание: {e}")))?;
    if !res.status.success() {
        let err = String::from_utf8_lossy(&res.stderr);
        let err = err.trim();
        return Err(ScannerError::Other(format!(
            "подписание не удалось (код {:?}): {}",
            res.status.code(),
            if err.is_empty() { "см. журнал инструмента" } else { err }
        )));
    }
    let output = if out.exists() { Some(out) } else { None };
    Ok(SignOutcome { output, command: cmd })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_substitution() {
        let cmd = build_command("cryptcp -signf {file}", Path::new("/a/b.pdf"), Path::new("/a/b-signed.pdf"));
        assert_eq!(cmd, "cryptcp -signf /a/b.pdf");
        let cmd = build_command("tool {file} {out}", Path::new("/a/b.pdf"), Path::new("/a/b-signed.pdf"));
        assert_eq!(cmd, "tool /a/b.pdf /a/b-signed.pdf");
    }

    #[test]
    fn file_appended_without_placeholders() {
        let cmd = build_command("signer -detached", Path::new("/a/b.pdf"), Path::new("/a/b-signed.pdf"));
        assert_eq!(cmd, "signer -detached /a/b.pdf");
    }

    #[test]
    fn sign_runs_simple_command() {
        // Дымовый тест на реальном sh: cat копирует файл в {out}.
        let dir = std::env::temp_dir().join(format!("sign-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("doc.pdf");
        std::fs::write(&pdf, b"%PDF-1.5 test").unwrap();
        let outcome = sign_pdf("cp {file} {out}", &pdf).unwrap();
        let out = outcome.output.expect("файл должен быть создан");
        assert!(out.to_string_lossy().ends_with("doc-signed.pdf"));
        assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sign_fails_on_bad_exit() {
        let dir = std::env::temp_dir().join(format!("sign-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("doc.pdf");
        std::fs::write(&pdf, b"x").unwrap();
        assert!(sign_pdf("false {file}", &pdf).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
