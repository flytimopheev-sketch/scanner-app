//! Сборка GUI: упаковка встроенных symbolic-иконок в GResource.
//!
//! Иконки, отсутствующие в системной теме Adwaita (обрезка, авто-выправление),
//! кладутся в ресурс с раскладкой темы и подключаются через IconTheme::add_resource_path.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=resources");
    println!("cargo:rerun-if-changed=resources/icons.xml");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let target = out_dir.join("icons.gresource");

    let ok = Command::new("glib-compile-resources")
        .current_dir(&manifest)
        .arg("--sourcedir=resources")
        .arg(format!("--target={}", target.display()))
        .arg("resources/icons.xml")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        // Резерв: пустой файл — регистрация ресурса в рантайме просто не сработает.
        std::fs::write(&target, b"").ok();
        println!(
            "cargo:warning=glib-compile-resources недоступен: встроенные иконки не добавлены"
        );
    }
}
