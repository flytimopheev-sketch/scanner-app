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

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Режим снимков под Xvfb: Vulkan-рендерер не отрисовывает вторичные окна
    // (диалоги мапятся, но остаются пустыми). Программный cairo — надёжен.
    if std::env::var_os("SCANNER_SHOTS").is_some_and(|v| !v.is_empty()) {
        std::env::set_var("GSK_RENDERER", "cairo");
    }

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
