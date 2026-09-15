//! Встроенные иконки (GResource).
//!
//! В минимальных темах (и в старых Adwaita) нет подходящих глифов для
//! «Обрезать» и «Выправить» — рисуем их сами и регистрируем в теме иконок.
//!
//! ВАЖНО про раскладку: GTK 4.10+ сканирует зарегистрированный resource-path
//! БЕЗ обхода подкаталогов — файлы иконок должны лежать плоско в корне пути
//! (gtkicontheme.c: add_unthemed_icon по g_resources_enumerate_children).

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;

const RESOURCE_PREFIX: &str = "/ru/redos/ScannerApp/icons";

pub fn register() {
    let bytes =
        glib::Bytes::from_static(include_bytes!(concat!(env!("OUT_DIR"), "/icons.gresource")));
    let Ok(resource) = gio::Resource::from_data(&bytes) else {
        return;
    };
    gio::resources_register(&resource);
    if let Some(display) = gdk::Display::default() {
        gtk4::IconTheme::for_display(&display).add_resource_path(RESOURCE_PREFIX);
    }
}
