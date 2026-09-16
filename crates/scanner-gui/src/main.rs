//! Сканер документов — GUI (GTK4 / libadwaita).
//!
//! GUI не общается с SANE напрямую: все операции выполняет отдельный
//! процесс scanner-worker (защита от зависаний и падений драйверов).

#![allow(deprecated)] // FileChooserDialog — совместимость с GTK 4.6 (РЕД ОС)

mod app;
mod icons;
mod shots;
mod state;
mod ui;
mod worker;

use gtk4::prelude::*;
use libadwaita as adw;
use gtk4 as gtk;
use scanner_core::i18n::{t, Lang};

/// Какой рендерер GTK4 использовать.
///
/// Аппаратные рендереры (vulkan/gl) на части систем РЕД ОС рисуют пустые
/// виджеты (окно открывается, но кнопок и списков не видно) и способны
/// подвесить графическую сессию целиком, поэтому по умолчанию включается
/// программный `cairo`.
///
/// * `renderer_set` — задал ли пользователь `GSK_RENDERER` сам (тогда не
///   вмешиваемся);
/// * `scanner_gl` — значение `SCANNER_GL`; `SCANNER_GL=1` требует
///   аппаратный рендеринг.
///
/// Возвращает значение для `GSK_RENDERER` либо `None`, если переменную
/// трогать не нужно.
fn renderer_override(renderer_set: bool, scanner_gl: Option<&str>) -> Option<&'static str> {
    if renderer_set {
        return None;
    }
    if scanner_gl == Some("1") {
        return None;
    }
    Some("cairo")
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Режим снимков под Xvfb: Vulkan-рендерер не отрисовывает вторичные окна
    // (диалоги мапятся, но остаются пустыми). Программный cairo — надёжен.
    if std::env::var_os("SCANNER_SHOTS").is_some_and(|v| !v.is_empty()) {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    // Аппаратные рендереры GTK4 (vulkan/gl) на некоторых системах РЕД ОС
    // вызывают зависание всей сессии (вплоть до гибели шины dbus) и рисуют
    // пустые виджеты. По умолчанию — программный cairo; вернуть аппаратный
    // рендеринг можно, задав SCANNER_GL=1.
    if let Some(value) = renderer_override(
        std::env::var_os("GSK_RENDERER").is_some(),
        std::env::var_os("SCANNER_GL").as_deref().and_then(|v| v.to_str()),
    ) {
        std::env::set_var("GSK_RENDERER", value);
    }
    // Выбранный рендерер — в журнал: без него причина «пустого окна» не видна.
    scanner_core::config::append_log(&format!(
        "renderer: GSK_RENDERER={} SCANNER_GL={}",
        std::env::var("GSK_RENDERER").unwrap_or_else(|_| "auto".into()),
        std::env::var("SCANNER_GL").unwrap_or_else(|_| "0".into()),
    ));

    let app = adw::Application::builder()
        .application_id("ru.redos.ScannerApp")
        .flags(gio::ApplicationFlags::empty())
        .build();

    app.connect_activate(|app| {
        // Тема приложения (мягкая современная поверх libadwaita).
        let provider = gtk::CssProvider::new();
        provider.load_from_data(ui::STYLE_CSS);
        let dark_provider = gtk::CssProvider::new();
        dark_provider.load_from_data(ui::STYLE_DARK_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );

            // Тёмная палитра: подключаем/отключаем следом за StyleManager::dark.
            let dark_state = std::rc::Rc::new(std::cell::Cell::new(false));
            {
                let display = display.clone();
                adw::StyleManager::default().connect_dark_notify(move |sm| {
                    let dark = sm.is_dark();
                    if dark == dark_state.get() {
                        return;
                    }
                    dark_state.set(dark);
                    if dark {
                        gtk::style_context_add_provider_for_display(
                            &display,
                            &dark_provider,
                            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
                        );
                    } else {
                        gtk::style_context_remove_provider_for_display(&display, &dark_provider);
                    }
                });
            }
        }

        // Журнал ошибок в файл рядом с данными приложения.
        scanner_core::config::append_log(&format!(
            "=== {} v{} ===",
            t(Lang::Ru, "app.title"),
            env!("CARGO_PKG_VERSION")
        ));

        if let Err(e) = app::run(app) {
            log::error!("gui init: {e}");
            scanner_core::config::append_log(&format!("gui init: {e}"));
            std::process::exit(1);
        }
    });

    app.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::renderer_override;

    /// Обычный запуск (ярлык из меню, терминал) — программный cairo:
    /// аппаратный рендерер на части систем РЕД ОС оставляет окно пустым
    /// и может подвесить сессию.
    #[test]
    fn default_is_software_cairo() {
        assert_eq!(renderer_override(false, None), Some("cairo"));
    }

    /// SCANNER_GL=1 — явный запрос аппаратного рендеринга.
    #[test]
    fn scanner_gl_enables_hardware() {
        assert_eq!(renderer_override(false, Some("1")), None);
    }

    /// Любое другое значение SCANNER_GL аппаратный рендеринг не включает.
    #[test]
    fn other_scanner_gl_values_keep_cairo() {
        for value in ["0", "", "yes", "true"] {
            assert_eq!(renderer_override(false, Some(value)), Some("cairo"));
        }
    }

    /// Свой GSK_RENDERER всегда важнее наших настроек.
    #[test]
    fn user_renderer_wins() {
        assert_eq!(renderer_override(true, None), None);
        assert_eq!(renderer_override(true, Some("1")), None);
        assert_eq!(renderer_override(true, Some("0")), None);
    }
}
