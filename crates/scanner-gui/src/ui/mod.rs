//! UI-модуль: диалоги и тема.

pub mod dialogs;

/// Мягкая современная тема: скругления 12px, пастельные тени,
/// светлые градиенты поверх libadwaita.
pub const STYLE_CSS: &str = include_str!("style.css");

/// Тёмная палитра: переопределяет переменные STYLE_CSS.
/// Подключается при StyleManager::dark == true (см. app::run).
pub const STYLE_DARK_CSS: &str = include_str!("style-dark.css");
