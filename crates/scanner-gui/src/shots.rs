//! Скрытый режим снимков интерфейса (для документации и скриншотов).
//!
//! Включается переменной окружения `SCANNER_SHOTS` = каталог для PNG-файлов.
//! Последовательность: главное окно → диалоги → сканирование (mock) →
//! галерея → предпросмотр → тёмная тема. После завершения приложение
//! закрывается само.

use std::path::PathBuf;

use gtk4::prelude::*;
use gtk4 as gtk;

use crate::app::Ctx;

pub fn maybe_run(ctx: &Ctx) {
    let dir = match std::env::var("SCANNER_SHOTS") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => return,
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("shots: {}: {e}", dir.display());
        return;
    }
    log::info!("shots: снимки будут в {}", dir.display());

    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        run_sequence(ctx, dir).await;
    });
}

async fn wait(ms: u64) {
    glib::timeout_future(std::time::Duration::from_millis(ms)).await;
}

async fn run_sequence(ctx: Ctx, dir: PathBuf) {
    let win: gtk::Window = ctx.ui.window.clone().upcast();

    // Под Xvfb без событий ввода GTK может не генерировать кадры:
    // CSS-переходы и смена темы «замирают». Тик по ВСЕМ окнам заставляет
    // frame clock работать — иначе диалоги мапятся, но остаются пустыми.
    {
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            for w in gtk::Window::list_toplevels() {
                w.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    let act = |name: &str, arg: Option<&glib::Variant>| {
        // Действия окна живут в группе "win", нужен префикс.
        let full = format!("win.{name}");
        if let Err(e) = gtk::prelude::WidgetExt::activate_action(&win, &full, arg) {
            eprintln!("shots: activate {full}: {e}");
        }
    };

    // В песочнице без dbus-сессии libadwaita может выбрать тёмную тему —
    // для снимков фиксируем светлую (тёмная показана отдельно в шаге 7).
    act("theme", Some(&"light".to_variant()));
    let shot = |name: &str, w: &gtk::Window| {
        // Без оконного менеджера Xvfb новое окно может остаться в нижней
        // части стека — поднимаем снимаемое окно перед захватом.
        w.present();
        let path = dir.join(name);
        capture(w, &path);
        println!("shots: {}", path.display());
    };

    // 1. Главное окно, светлая тема, пустое состояние.
    // Первый запуск под Xvfb холодный (кэши шрифтов/иконок) — ждём подольше;
    // заодно успевает исчезнуть тост «Тема изменена» (живёт 5 с).
    wait(4000).await;
    wait(3000).await;
    shot("01-main-window.png", &win);

    // 2. Диалог добавления устройства.
    act("add-device", None);
    wait(1800).await;
    if let Some(top) = topmost_other(&win) {
        shot("02-add-device.png", &top);
    }
    close_others(&win);

    // 3. Управление профилями (сначала сохраним демо-профили для наглядности).
    wait(200).await;
    {
        let presets: [&str; 2] = ["Документы — цвет, 300 DPI", "Пакет PDF — ч/б, 200 DPI"];
        {
            let mut st = ctx.state.borrow_mut();
            st.options.source = scanner_core::ScanSource::Flatbed;
            st.options.resolution = 300;
            st.options.mode = scanner_core::ScanMode::Color;
        }
        crate::app::save_profile_named(&ctx, presets[0]);
        {
            let mut st = ctx.state.borrow_mut();
            st.options.source = scanner_core::ScanSource::Adf;
            st.options.resolution = 200;
            st.options.mode = scanner_core::ScanMode::Gray;
        }
        crate::app::save_profile_named(&ctx, presets[1]);
    }
    wait(600).await;
    act("profile-manage", None);
    wait(1800).await;
    if let Some(top) = topmost_other(&win) {
        shot("03-profiles.png", &top);
    }
    close_others(&win);

    // 4. Сканирование ADF (mock) → галерея из четырёх страниц.
    wait(200).await;
    ctx.ui.dd_source.set_selected(1); // Податчик (ADF)
    ctx.state.borrow_mut().options.source = scanner_core::ScanSource::Adf;
    wait(400).await;
    act("scan", None);
    wait(16000).await;

    // 4a. Авто-выправление всех страниц (deskew + авто-обрезка) —
    // демонстрирует новую панель действий. Ждём завершения: тост исчезнет,
    // эскизы успеют перерисоваться.
    act("autofix-all", None);
    wait(14000).await;
    wait(11000).await;
    shot("04-gallery.png", &win);

    // 5. Предпросмотр выбранной страницы (рендер в debug небыстрый).
    wait(500).await;
    act("preview", None);
    wait(2500).await;
    wait(3500).await;
    if let Some(top) = topmost_other(&win) {
        shot("05-preview.png", &top);
    }
    close_others(&win);

    // 6. О программе.
    wait(200).await;
    act("about", None);
    wait(1800).await;
    if let Some(top) = topmost_other(&win) {
        shot("06-about.png", &top);
    }
    close_others(&win);

    // 7. Тёмная тема (тост о смене темы тоже попадёт в кадр).
    wait(200).await;
    act("theme", Some(&"dark".to_variant()));
    wait(1100).await;
    shot("07-dark-theme.png", &win);

    // Завершение.
    wait(400).await;
    if let Some(app) = win.application() {
        app.quit();
    }
}

/// Снимок окна в PNG. В Xvfb без оконного менеджера стек окон ненадёжен
/// (диалог мапится ПОД главное окно, present() не помогает), поэтому
/// окно ищется по заголовку через xwininfo и снимается напрямую (xwd -id).
/// Если xwininfo недоступен или окно не найдено — захват всего экрана.
fn capture(window: &gtk::Window, path: &std::path::Path) {
    let xwd_path = path.with_extension("xwd");
    let mut xwd_args: Vec<String> = vec!["-root".into(), "-silent".into()];

    if let Some(title) = window.title() {
        if let Ok(out) = std::process::Command::new("xwininfo")
            .args(["-root", "-tree"])
            .output()
        {
            let tree = String::from_utf8_lossy(&out.stdout);
            for line in tree.lines() {
                let t = line.trim_start();
                if t.starts_with("0x") && line.contains(title.as_str()) {
                    if let Some(id) = t.split_whitespace().next() {
                        xwd_args = vec!["-id".into(), id.to_string(), "-silent".into()];
                        break;
                    }
                }
            }
        }
    }

    let dump = std::process::Command::new("xwd")
        .args(&xwd_args)
        .arg("-out")
        .arg(&xwd_path)
        .status();
    match dump {
        Ok(s) if s.success() => {}
        Ok(s) => {
            log::warn!("shots: xwd завершился с {s}");
            return;
        }
        Err(e) => {
            log::warn!("shots: xwd недоступен: {e}");
            return;
        }
    }
    let conv = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "xwdtopnm '{}' | pnmtopng > '{}'",
            xwd_path.display(),
            path.display()
        ))
        .status();
    let _ = std::fs::remove_file(&xwd_path);
    if let Err(e) = conv {
        log::warn!("shots: xwdtopnm/pnmtopng: {e}");
    }
}

/// Верхнее видимое окно, кроме главного (открытый диалог), если есть.
fn topmost_other(main: &gtk::Window) -> Option<gtk::Window> {
    let main_obj = main.upcast_ref::<glib::Object>();
    gtk::Window::list_toplevels()
        .into_iter()
        .filter(|w| w.is_visible() && w.upcast_ref::<glib::Object>() != main_obj)
        .find_map(|w| w.downcast::<gtk::Window>().ok())
}

/// Закрыть все служебные окна, кроме главного.
fn close_others(main: &gtk::Window) {
    let main_obj = main.upcast_ref::<glib::Object>();
    for w in gtk::Window::list_toplevels() {
        if w.is_visible() && w.upcast_ref::<glib::Object>() != main_obj {
            if let Ok(win) = w.downcast::<gtk::Window>() {
                win.close();
            }
        }
    }
}
