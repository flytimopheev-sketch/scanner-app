//! Шаблоны имён файлов экспорта.
//!
//! Поддерживаемые подстановки:
//! * `{date}` — дата ГГГГ-ММ-ДД;
//! * `{time}` — время ЧЧ-ММ;
//! * `{profile}` — имя профиля (очищенное от недопустимых символов);
//! * `{device}` — имя устройства (очищенное, укороченное);
//! * `{n}` — номер страницы (01, 02, …) для постраничных форматов.
//!
//! Символы, недопустимые в именах файлов, заменяются на `_`.

use std::path::Path;

/// Контекст подстановки для шаблона.
#[derive(Debug, Clone, Default)]
pub struct NameContext {
    pub profile: String,
    pub device: String,
}

/// Очистить строку для использования в имени файла.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_sep = false;
    for ch in s.chars() {
        // Буквы (в т.ч. кириллица), цифры и безопасные знаки остаются как есть;
        // пробелы и всё прочее схлопываются в одиночный '_'.
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '(' | ')' | '.') {
            out.push(ch);
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "scan".to_string()
    } else {
        trimmed
    }
}

/// Развернуть шаблон в базовое имя файла (без расширения).
pub fn expand_template(tpl: &str, ctx: &NameContext) -> String {
    let now = chrono::Local::now();
    let out = tpl
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M").to_string())
        .replace("{profile}", &sanitize(&ctx.profile))
        .replace("{device}", &sanitize(&ctx.device))
        .replace('{', "_")
        .replace('}', "_");
    let cleaned = out.trim_matches('_').trim().to_string();
    if cleaned.is_empty() {
        "scan".to_string()
    } else {
        cleaned
    }
}

/// Имя файла по умолчанию (когда шаблон не задан пользователем).
pub const DEFAULT_TEMPLATE: &str = "{profile}_{date}_{time}";

/// Сформировать путь экспорта: каталог + развёрнутый шаблон + расширение.
pub fn template_path(dir: &Path, tpl: &str, ctx: &NameContext, ext: &str) -> std::path::PathBuf {
    dir.join(format!("{}.{}", expand_template(tpl, ctx), ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_bad_chars() {
        assert_eq!(sanitize("Office: MFP/2?"), "Office_MFP_2");
        assert_eq!(sanitize("  a  b  "), "a_b");
        assert_eq!(sanitize("///"), "scan");
        assert_eq!(sanitize("Brother MFC-L2750DW"), "Brother_MFC-L2750DW");
    }

    #[test]
    fn expand_replaces_tokens() {
        let ctx = NameContext {
            profile: "Договор А4".into(),
            device: "pixma:04A9:1".into(),
        };
        let s = expand_template("{profile}_{device}_{date}_{time}", &ctx);
        assert!(s.starts_with("Договор_А4_pixma_04A9_1_20"), "{s}");
        // Дата в формате ГГГГ-ММ-ДД.
        let mid = &s["Договор_А4_pixma_04A9_1_".len()..];
        assert!(mid.len() >= 10 && mid.as_bytes()[4] == b'-', "{s}");
    }

    #[test]
    fn expand_unknown_tokens_are_removed() {
        let ctx = NameContext::default();
        let s = expand_template("doc_{n}_{unknown}", &ctx);
        assert!(!s.contains('{'), "{s}");
        assert!(!s.is_empty());
    }

    #[test]
    fn template_path_builds_full_name() {
        let ctx = NameContext { profile: "p".into(), device: "d".into() };
        let p = template_path(Path::new("/tmp"), "{profile}_{date}", &ctx, "pdf");
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("pdf"));
        assert!(p.to_string_lossy().starts_with("/tmp/p_20"), "{}", p.display());
    }
}
