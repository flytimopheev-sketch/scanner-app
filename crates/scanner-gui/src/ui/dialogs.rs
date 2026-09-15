//! Модальные диалоги: добавление устройства, профили, обрезка, журнал, просмотр.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use adw::prelude::*;
use scanner_core::i18n::Lang;
use scanner_core::imageproc::CropRect;
use scanner_core::i18n::t;
use scanner_core::config;

use crate::app::{AppCtx, UiMsg};
use crate::state::{render_thumb, edited_cache_path};

/// Каркас диалога: окно + AdwHeaderBar + вертикальный контейнер.
/// GtkDialog в GTK4 не умеет заголовки, поэтому все диалоги — окна.
fn dialog_shell() -> (gtk::Window, gtk::Box, adw::HeaderBar) {
    let win = gtk::Window::new();
    let header = adw::HeaderBar::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&content);
    win.set_child(Some(&root));
    (win, content, header)
}

/// Кнопка «Закрыть/Отмена» в шапке, закрывающая окно.
fn header_close(header: &adw::HeaderBar, dialog: &gtk::Window, label: &str, suggested: bool) {
    let btn = gtk::Button::with_label(label);
    if suggested {
        btn.add_css_class("suggested-action");
    } else {
        btn.add_css_class("flat");
    }
    let dlg = dialog.clone();
    btn.connect_clicked(move |_| dlg.close());
    header.pack_end(&btn);
}

/// Ручки виджетов диалога добавления устройства.
pub struct AddDeviceUi {
    pub dialog: gtk::Window,
    pub found_list: gtk::ListBox,
    pub ip_entry: gtk::Entry,
    pub name_entry: gtk::Entry,
    pub dd_proto: gtk::DropDown,
    pub result_label: gtk::Label,
    pub check_btn: gtk::Button,
    pub save_btn: gtk::Button,
    pub snippet_label: gtk::Label,
    pub spinner: gtk::Spinner,
}

/// Диалог «Добавить устройство».
pub fn show_add_device(ctx: &Rc<AppCtx>) -> Rc<AddDeviceUi> {
    let lang = ctx.state.borrow().lang;
    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "add_device.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(560, 480);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_spacing(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    switcher.set_halign(gtk::Align::Center);

    content.append(&switcher);
    content.append(&stack);

    // ---- Страница «Автопоиск» ----
    let auto_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let hint = gtk::Label::new(Some(&t(lang, "add_device.auto.hint")));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("dimmed");
    auto_box.append(&hint);

    let find_btn = gtk::Button::with_label(&t(lang, "device.refresh"));
    find_btn.add_css_class("suggested-action");
    let found_list = gtk::ListBox::new();
    found_list.add_css_class("boxed-list");
    found_list.set_selection_mode(gtk::SelectionMode::None);
    let found_scroll = gtk::ScrolledWindow::new();
    found_scroll.set_child(Some(&found_list));
    found_scroll.set_vexpand(true);
    found_scroll.set_min_content_height(220);
    auto_box.append(&find_btn);
    auto_box.append(&found_scroll);
    let auto_title = t(lang, "add_device.tab.auto");
    stack.add_titled(&auto_box, Some("auto"), auto_title.as_str());

    // ---- Страница «По IP» ----
    let ip_box = gtk::Box::new(gtk::Orientation::Vertical, 10);

    let ip_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let ip_label = gtk::Label::new(Some(&t(lang, "add_device.ip.address")));
    ip_label.set_xalign(0.0);
    ip_label.set_width_chars(16);
    let ip_entry = gtk::Entry::new();
    ip_entry.set_placeholder_text(Some("192.168.1.50"));
    ip_entry.set_hexpand(true);
    ip_row.append(&ip_label);
    ip_row.append(&ip_entry);
    ip_box.append(&ip_row);

    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name_label = gtk::Label::new(Some(&t(lang, "add_device.ip.name")));
    name_label.set_xalign(0.0);
    name_label.set_width_chars(16);
    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Office MFP"));
    name_entry.set_hexpand(true);
    name_row.append(&name_label);
    name_row.append(&name_entry);
    ip_box.append(&name_row);

    let proto_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let proto_label = gtk::Label::new(Some(&t(lang, "add_device.ip.protocol")));
    proto_label.set_xalign(0.0);
    proto_label.set_width_chars(16);
    let proto_auto = t(lang, "add_device.ip.proto.auto");
    let dd_proto = gtk::DropDown::from_strings(&[proto_auto.as_str(), "eSCL", "WSD"]);
    proto_row.append(&proto_label);
    proto_row.append(&dd_proto);
    ip_box.append(&proto_row);

    let check_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let check_btn = gtk::Button::with_label(&t(lang, "add_device.ip.check"));
    check_btn.add_css_class("suggested-action");
    let spinner = gtk::Spinner::new();
    let result_label = gtk::Label::new(None);
    result_label.set_hexpand(true);
    result_label.set_xalign(0.0);
    result_label.set_wrap(true);
    check_row.append(&check_btn);
    check_row.append(&spinner);
    check_row.append(&result_label);
    ip_box.append(&check_row);

    // Подсказка с airscan.conf для постоянной настройки.
    let snippet_label = gtk::Label::new(None);
    snippet_label.set_wrap(true);
    snippet_label.set_xalign(0.0);
    snippet_label.add_css_class("monospace");
    snippet_label.add_css_class("dimmed");
    let copy_btn = gtk::Button::with_label(&t(lang, "add_device.ip.copy"));
    copy_btn.set_halign(gtk::Align::Start);
    ip_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    ip_box.append(&snippet_label);
    ip_box.append(&copy_btn);

    let save_btn = gtk::Button::with_label(&t(lang, "app.save"));
    save_btn.add_css_class("suggested-action");
    save_btn.add_css_class("pill");
    save_btn.set_halign(gtk::Align::End);
    save_btn.set_sensitive(false);
    ip_box.append(&save_btn);

    let ip_title = t(lang, "add_device.tab.ip");
    stack.add_titled(&ip_box, Some("ip"), ip_title.as_str());

    dialog.present();

    let ui = Rc::new(AddDeviceUi {
        dialog: dialog.clone(),
        found_list,
        ip_entry,
        name_entry,
        dd_proto,
        result_label,
        check_btn,
        save_btn,
        snippet_label,
        spinner,
    });

    // Поиск устройств (тот же запрос, что и в главном окне).
    {
        let ctx = ctx.clone();
        find_btn.connect_clicked(move |_| {
            crate::app::refresh_devices(&ctx);
        });
    }

    // Проверка подключения по IP.
    {
        let ctx = ctx.clone();
        let ui = ui.clone();
        let check_btn = ui.check_btn.clone();
        check_btn.connect_clicked(move |_| {
            let lang = ctx.state.borrow().lang;
            let ip = ui.ip_entry.text().to_string();
            if ip.trim().is_empty() {
                ui.result_label.set_text(&t(lang, "add_device.ip.unreachable"));
                return;
            }
            let proto = proto_key(&ui.dd_proto);
            ui.result_label.set_text(&t(lang, "add_device.ip.checking"));
            ui.spinner.start();
            ui.check_btn.set_sensitive(false);
            let id = ctx
                .worker
                .request(scanner_core::Request::TestIp {
                    address: ip.trim().to_string(),
                    protocol: Some(proto.to_string()),
                });
            ctx.pending
                .borrow_mut()
                .insert(id, crate::app::Pending::TestIp(ui.clone()));
        });
    }

    // Копирование сниппета.
    {
        let ui = ui.clone();
        let ctx = ctx.clone();
        let btn = copy_btn.clone();
        copy_btn.connect_clicked(move |_| {
            let lang = ctx.state.borrow().lang;
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(&ui.snippet_label.text());
            }
            btn.set_label(&t(lang, "add_device.ip.copied"));
        });
    }

    // Сохранение устройства.
    {
        let ctx = ctx.clone();
        let ui = ui.clone();
        let save_btn = ui.save_btn.clone();
        save_btn.connect_clicked(move |_| {
            save_manual_device(&ctx, &ui);
        });
    }

    // Обновить сниппет при вводе.
    {
        let ip_entry = ui.ip_entry.clone();
        let name_entry = ui.name_entry.clone();
        let dd_proto = ui.dd_proto.clone();
        let ui_c = ui.clone();
        let ctx_c = ctx.clone();
        let update = move || {
            let lang = ctx_c.state.borrow().lang;
            update_snippet(&ui_c, lang);
        };
        ip_entry.connect_changed({
            let update = update.clone();
            move |_| update()
        });
        name_entry.connect_changed({
            let update = update.clone();
            move |_| update()
        });
        dd_proto.connect_selected_notify({
            let update = update.clone();
            move |_| update()
        });
        update();
    }

    // Выбор найденного устройства из списка.
    {
        let ctx = ctx.clone();
        let ui = ui.clone();
        let list = ui.found_list.clone();
        list.connect_row_activated(move |_, row| {
            let idx = row.index().max(0) as usize;
            let devices = ctx.state.borrow().devices.clone();
            if let Some(dev) = devices.get(idx) {
                crate::app::select_device(&ctx, dev.clone());
                ui.dialog.close();
            }
        });
    }

    ui
}

fn proto_key(dd: &gtk::DropDown) -> &'static str {
    match dd.selected() {
        1 => "escl",
        2 => "wsd",
        _ => "auto",
    }
}

fn update_snippet(ui: &AddDeviceUi, lang: Lang) {
    let ip = ui.ip_entry.text().trim().to_string();
    let name = ui.name_entry.text().trim().to_string();
    let alias = if name.is_empty() { "Office MFP" } else { &name };
    let port = ip.split_once(':').map(|(_, p)| p.to_string()).unwrap_or_else(|| "8080".into());
    let host = ip.split(':').next().unwrap_or(&ip).to_string();
    let text = if ip.is_empty() {
        format!(
            "# {}: /etc/sane.d/airscan.conf\ndevice \"{alias}\" {{\n    uri = \"escl:http://192.168.1.50:8080\"\n}}",
            t(lang, "add_device.ip.config_hint")
        )
    } else {
        format!(
            "device \"{alias}\" {{\n    uri = \"escl:http://{host}:{port}\"\n}}"
        )
    };
    ui.snippet_label.set_text(&text);
}

fn save_manual_device(ctx: &Rc<AppCtx>, ui: &AddDeviceUi) {
    use scanner_core::device::{short_id, DeviceKind, ScannerDevice};
    let lang = ctx.state.borrow().lang;

    let raw = ui.ip_entry.text().trim().to_string();
    if raw.is_empty() {
        return;
    }
    let (host, port) = match raw.split_once(':') {
        Some((h, p)) => (h.to_string(), p.to_string()),
        None => (raw.clone(), "8080".to_string()),
    };
    let proto = proto_key(&ui.dd_proto).to_string();
    let proto_uri = if proto == "wsd" { "wsd" } else { "escl" };
    let name_text = ui.name_entry.text().trim().to_string();
    let display = if name_text.is_empty() {
        format!("{host} ({proto_uri})")
    } else {
        name_text.clone()
    };

    let device_name = format!("manual|{proto_uri}|{host}|{port}|{display}");
    let dev = ScannerDevice {
        id: short_id(&device_name),
        name: display.clone(),
        vendor: None,
        model: Some(display),
        kind: DeviceKind::Network,
        backend: "airscan".into(),
        device_name,
        address: Some(format!("{host}:{port}")),
        protocol: Some(proto_uri.to_string()),
    };

    if let Some(store) = &ctx.state.borrow().store {
        let _ = store.save_device(&dev);
    }
    crate::app::add_manual_device(ctx, dev);
    ui.dialog.close();
    ctx.ui_msg_tx
        .send_blocking(UiMsg::Toast(t(lang, "add_device.saved")))
        .ok();
}

/// Диалог сохранения профиля.
pub fn show_save_profile(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;
    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "profiles.save")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(420, 140);
    let save_btn = gtk::Button::with_label(&t(lang, "app.save"));
    save_btn.add_css_class("suggested-action");
    header.pack_end(&save_btn);
    let cancel_btn = gtk::Button::with_label(&t(lang, "app.cancel"));
    cancel_btn.add_css_class("flat");
    header.pack_start(&cancel_btn);

    content.set_spacing(10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let label = gtk::Label::new(Some(&t(lang, "profiles.name")));
    label.set_xalign(0.0);
    let entry = gtk::Entry::new();
    content.append(&label);
    content.append(&entry);
    dialog.set_default_widget(Some(&entry));

    {
        let ctx = ctx.clone();
        let entry = entry.clone();
        let dialog_c = dialog.clone();
        save_btn.connect_clicked(move |_| {
            let name = entry.text().trim().to_string();
            if !name.is_empty() {
                crate::app::save_profile_named(&ctx, &name);
            }
            dialog_c.close();
        });
    }
    {
        let ctx = ctx.clone();
        let dialog_c = dialog.clone();
        entry.connect_activate(move |e| {
            let name = e.text().trim().to_string();
            if !name.is_empty() {
                crate::app::save_profile_named(&ctx, &name);
            }
            dialog_c.close();
        });
    }
    {
        let dialog_c = dialog.clone();
        cancel_btn.connect_clicked(move |_| dialog_c.close());
    }
    dialog.present();
    entry.grab_focus();
}

/// Диалог управления профилями (список + удаление).
pub fn show_profiles(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;
    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "profiles.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(460, 380);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_spacing(10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);
    content.append(&scroll);

    let profiles = ctx.state.borrow().profiles.clone();
    if profiles.is_empty() {
        let empty = gtk::Label::new(Some(&t(lang, "profiles.empty")));
        empty.add_css_class("dimmed");
        empty.set_margin_top(12);
        list.append(&empty);
    }
    for p in profiles {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&p.name));
        name.set_hexpand(true);
        name.set_xalign(0.0);
        let info = gtk::Label::new(Some(&format!(
            "{} dpi · {} · {}",
            p.options.resolution,
            p.options.mode.key(),
            p.options.format.key()
        )));
        info.add_css_class("dimmed");
        let del = gtk::Button::from_icon_name("user-trash-symbolic");
        del.add_css_class("flat");
        let pid = p.id;
        let ctx = ctx.clone();
        del.connect_clicked(move |_| {
            if let Some(id) = pid {
                if let Some(store) = &ctx.state.borrow().store {
                    let _ = store.delete_profile(id);
                }
                ctx.state.borrow_mut().reload_profiles();
                crate::app::refresh_profiles(&ctx);
            }
        });
        row.append(&name);
        row.append(&info);
        row.append(&del);
        let wrapper = gtk::ListBoxRow::new();
        wrapper.set_child(Some(&row));
        list.append(&wrapper);
    }

    dialog.present();
}

/// Диалог обрезки выбранной страницы.
///
/// Прямоугольник обрезки интерактивный: за углы рамку можно тянуть,
/// внутри рамки — двигать целиком; слайдеры остаются как запасной
/// способ точной подстройки.
pub fn show_crop(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;
    let (idx, page) = {
        let state = ctx.state.borrow();
        match state.selected {
            Some(i) if i < state.pages.len() => (i, state.pages[i].clone()),
            _ => return,
        }
    };

    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "crop.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(560, 680);
    header_close(&header, &dialog, &t(lang, "app.cancel"), false);

    content.set_spacing(10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    // Превью с прямоугольником обрезки.
    let overlay = gtk::Overlay::new();
    let pic = gtk::Picture::new();
    pic.set_size_request(420, 440);
    pic.set_can_shrink(true);
    overlay.set_child(Some(&pic));

    let area = gtk::DrawingArea::new();
    overlay.add_overlay(&area);
    content.append(&overlay);

    let hint = gtk::Label::new(Some(&t(lang, "crop.hint")));
    hint.add_css_class("dimmed");
    hint.add_css_class("caption");
    content.append(&hint);

    // Текущие значения: top, bottom, left, right (в процентах).
    let vals = Rc::new(RefCell::new([0.0f64, 0.0f64, 0.0f64, 0.0f64]));
    let aspect = Rc::new(RefCell::new(210.0f64 / 297.0f64));

    // Загрузить превью (применяем поворот/фильтр, но без обрезки).
    // Рендер откладываем на короткий таймер: диалог открывается мгновенно.
    {
        let pic = pic.clone();
        let aspect = aspect.clone();
        let src = page.source.clone();
        let mut preview_edit = page.edit;
        preview_edit.crop = None;
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let path = edited_cache_path(&src, &preview_edit, "crop");
            let rendered = render_thumb(&src, &preview_edit, &path);
            if rendered.is_ok() {
                if let Some(tex) = crate::state::load_texture(&path) {
                    let a = tex.width() as f64 / tex.height() as f64;
                    *aspect.borrow_mut() = a;
                    pic.set_paintable(Some(&tex));
                }
            }
            glib::ControlFlow::Break
        });
    }

    // Геометрия изображения внутри виджета (letterbox) — общая для
    // отрисовки и обработки жестов.
    let img_rect = |w: f64, h: f64, a: f64| -> (f64, f64, f64, f64) {
        let (img_w, img_h) = if w / h > a {
            ((h * a).floor(), h)
        } else {
            (w, ((w / a).floor()))
        };
        let x0 = ((w - img_w) / 2.0).max(0.0);
        let y0 = ((h - img_h) / 2.0).max(0.0);
        (x0, y0, img_w, img_h)
    };

    // Отрисовка прямоугольника + ручек по углам.
    {
        let vals = vals.clone();
        let aspect = aspect.clone();
        area.set_draw_func(move |_area, cr, w, h| {
            let v = vals.borrow();
            let a = *aspect.borrow();
            let (x0, y0, img_w, img_h) = img_rect(w as f64, h as f64, a);
            let top = y0 + img_h * v[0] / 100.0;
            let bottom = y0 + img_h * (1.0 - v[1] / 100.0);
            let left = x0 + img_w * v[2] / 100.0;
            let right = x0 + img_w * (1.0 - v[3] / 100.0);

            // Затемняем четыре полосы вокруг области кадрирования.
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            cr.rectangle(x0, y0, img_w, top - y0);
            let _ = cr.fill();
            cr.rectangle(x0, bottom, img_w, y0 + img_h - bottom);
            let _ = cr.fill();
            cr.rectangle(x0, top, left - x0, bottom - top);
            let _ = cr.fill();
            cr.rectangle(right, top, x0 + img_w - right, bottom - top);
            let _ = cr.fill();

            // Рамка.
            cr.set_source_rgba(0.95, 0.35, 0.35, 0.95);
            cr.set_line_width(2.0);
            cr.rectangle(left, top, right - left, bottom - top);
            let _ = cr.stroke();

            // Ручки по углам: белые квадраты с красной обводкой.
            const R: f64 = 6.0;
            for (hx, hy) in [
                (left, top),
                (right, top),
                (left, bottom),
                (right, bottom),
            ] {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                cr.rectangle(hx - R, hy - R, R * 2.0, R * 2.0);
                let _ = cr.fill();
                cr.set_source_rgba(0.95, 0.35, 0.35, 1.0);
                cr.set_line_width(2.0);
                cr.rectangle(hx - R, hy - R, R * 2.0, R * 2.0);
                let _ = cr.stroke();
            }
        });
    }

    // Слайдеры (синхронизируются с жестом мыши).
    let scales: Rc<RefCell<Vec<gtk::Scale>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let keys = ["crop.top", "crop.bottom", "crop.left", "crop.right"];
        for (i, key) in keys.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::new(Some(&t(lang, key)));
            label.set_width_chars(12);
            label.set_xalign(0.0);
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 45.0, 0.5);
            scale.set_value(0.0);
            scale.set_hexpand(true);
            let vals = vals.clone();
            let area = area.clone();
            scale.connect_value_changed(move |s| {
                vals.borrow_mut()[i] = s.value();
                area.queue_draw();
            });
            scales.borrow_mut().push(scale.clone());
            row.append(&label);
            row.append(&scale);
            content.append(&row);
        }
    }

    // Синхронизация значений -> слайдеры.
    let sync_scales = {
        let scales = scales.clone();
        let vals = vals.clone();
        move || {
            let s = scales.borrow();
            let v = *vals.borrow();
            for (i, scale) in s.iter().enumerate() {
                scale.set_value(v[i]);
            }
        }
    };

    // Интерактивный жест: перетаскивание углов и самой рамки.
    {
        let vals = vals.clone();
        let aspect = aspect.clone();
        let area = area.clone();
        let sync = sync_scales.clone();
        enum Mode {
            None,
            Corner(usize, usize), // (индекс top/bottom, индекс left/right)
            Move([f64; 4]),
        }
        let mode = Rc::new(RefCell::new(Mode::None));
        let begin = Rc::new(RefCell::new((0.0f64, 0.0f64)));

        let grab = {
            let vals = vals.clone();
            let aspect = aspect.clone();
            let mode = mode.clone();
            let begin = begin.clone();
            let area = area.clone();
            move |x: f64, y: f64| {
                // Позиция рамки вычисляется при отрисовке; здесь нужен
                // размер виджета — берём из allocated размеров.
                let w = area.width() as f64;
                let h = area.height() as f64;
                let a = *aspect.borrow();
                let (x0, y0, img_w, img_h) = img_rect(w, h, a);
                let v = *vals.borrow();
                let top = y0 + img_h * v[0] / 100.0;
                let bottom = y0 + img_h * (1.0 - v[1] / 100.0);
                let left = x0 + img_w * v[2] / 100.0;
                let right = x0 + img_w * (1.0 - v[3] / 100.0);
                *begin.borrow_mut() = (x, y);
                const GRAB: f64 = 26.0;
                let near = |px: f64, py: f64| (px - x).hypot(py - y) <= GRAB;
                let corners: [(usize, usize, f64, f64); 4] = [
                    (0, 2, left, top),    // TL: top+left
                    (0, 3, right, top),   // TR: top+right
                    (1, 2, left, bottom), // BL: bottom+left
                    (1, 3, right, bottom), // BR: bottom+right
                ];
                if let Some((vi, hi, _, _)) =
                    corners.iter().find(|(_, _, px, py)| near(*px, *py)).map(|c| (c.0, c.1, c.2, c.3))
                {
                    *mode.borrow_mut() = Mode::Corner(vi, hi);
                    return;
                }
                // Внутри рамки — двигаем целиком.
                if x >= left.min(right) && x <= left.max(right)
                    && y >= top.min(bottom) && y <= top.max(bottom)
                {
                    *mode.borrow_mut() = Mode::Move(v);
                } else {
                    *mode.borrow_mut() = Mode::None;
                }
            }
        };

        let gesture = gtk::GestureDrag::new();
        {
            let mode = mode.clone();
            let grab = grab.clone();
            gesture.connect_drag_begin(move |g, x, y| {
                grab(x, y);
                if matches!(*mode.borrow(), Mode::None) {
                    g.set_state(gtk::EventSequenceState::Denied);
                } else {
                    g.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        {
            let vals = vals.clone();
            let aspect = aspect.clone();
            let mode = mode.clone();
            let begin = begin.clone();
            let area_c = area.clone();
            let sync = sync.clone();
            gesture.connect_drag_update(move |_g, dx, dy| {
                let (bx, by) = *begin.borrow();
                let _ = (bx, by);
                let w = area_c.width() as f64;
                let h = area_c.height() as f64;
                let a = *aspect.borrow();
                let (_x0, _y0, img_w, img_h) = img_rect(w, h, a);
                if img_w < 1.0 || img_h < 1.0 {
                    return;
                }
                let dxp = dx / img_w * 100.0;
                let dyp = dy / img_h * 100.0;
                let mut v = *vals.borrow();
                match *mode.borrow() {
                    Mode::Corner(vi, hi) => {
                        // Верх/низ и лево/право — стороны, к которым прижат угол.
                        if vi == 0 {
                            v[0] = (v[0] + dyp).clamp(0.0, 95.0 - v[1]);
                        } else {
                            v[1] = (v[1] - dyp).clamp(0.0, 95.0 - v[0]);
                        }
                        if hi == 2 {
                            v[2] = (v[2] + dxp).clamp(0.0, 95.0 - v[3]);
                        } else {
                            v[3] = (v[3] - dxp).clamp(0.0, 95.0 - v[2]);
                        }
                    }
                    Mode::Move(orig) => {
                        let (ot, ob, ol, orr) = (orig[0], orig[1], orig[2], orig[3]);
                        let dt = (ot + dyp).clamp(0.0, 95.0 - ob);
                        v[0] = dt;
                        v[1] = (ob - dyp).clamp(0.0, 95.0 - v[0]);
                        let dl = (ol + dxp).clamp(0.0, 95.0 - orr);
                        v[2] = dl;
                        v[3] = (orr - dxp).clamp(0.0, 95.0 - v[2]);
                    }
                    Mode::None => {}
                }
                *vals.borrow_mut() = v;
                area_c.queue_draw();
                sync();
            });
        }
        area.add_controller(gesture);
    }

    // Кнопки.
    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let reset_btn = gtk::Button::with_label(&t(lang, "crop.reset"));
    {
        let vals = vals.clone();
        let area = area.clone();
        let sync = sync_scales.clone();
        reset_btn.connect_clicked(move |_| {
            *vals.borrow_mut() = [0.0; 4];
            area.queue_draw();
            sync();
        });
    }
    let apply_btn = gtk::Button::with_label(&t(lang, "app.apply"));
    apply_btn.add_css_class("suggested-action");
    apply_btn.add_css_class("pill");
    btn_row.append(&reset_btn);
    btn_row.append(&apply_btn);
    content.append(&btn_row);

    {
        let ctx = ctx.clone();
        let vals = vals.clone();
        let dialog_c = dialog.clone();
        apply_btn.connect_clicked(move |_| {
            let v = *vals.borrow();
            let crop = CropRect {
                top: (v[0] / 100.0) as f32,
                bottom: (v[1] / 100.0) as f32,
                left: (v[2] / 100.0) as f32,
                right: (v[3] / 100.0) as f32,
            };
            crate::app::set_page_crop(&ctx, idx, if crop.is_empty() { None } else { Some(crop) });
            dialog_c.close();
        });
    }

    dialog.present();
}

/// Диалог журнала ошибок.
pub fn show_log(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;
    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "settings.log")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(640, 440);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let text = gtk::TextView::new();
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_monospace(true);
    text.set_wrap_mode(gtk::WrapMode::None);

    let body = std::fs::read_to_string(config::gui_log_path()).unwrap_or_default();
    let body = if body.trim().is_empty() {
        t(lang, "log.empty")
    } else {
        body
    };
    text.buffer().set_text(&body);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&text));
    scroll.set_vexpand(true);
    content.append(&scroll);
    dialog.present();
}

/// Просмотр страницы крупным планом.
pub fn show_preview(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;
    let (idx, page) = {
        let state = ctx.state.borrow();
        let sel = state.selected.or(if state.pages.is_empty() { None } else { Some(0) });
        match sel {
            Some(i) if i < state.pages.len() => (i, state.pages[i].clone()),
            _ => {
                ctx.ui_msg_tx
                    .send_blocking(UiMsg::Toast(t(lang, "status.no_pages")))
                    .ok();
                return;
            }
        }
    };

    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&scanner_core::tf!(lang, "page.n", idx + 1)));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(760, 680);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);

    let pic = gtk::Picture::new();
    pic.set_size_request(700, 600);
    pic.set_can_shrink(true);
    content.append(&pic);

    // Рендерим с короткой задержкой, чтобы окно открылось мгновенно.
    {
        let src = page.source.clone();
        let edit = page.edit;
        let pic = pic.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let path = edited_cache_path(&src, &edit, "preview");
            let rendered = render_thumb(&src, &edit, &path);
            if rendered.is_ok() {
                if let Some(tex) = crate::state::load_texture(&path) {
                    pic.set_paintable(Some(&tex));
                }
            }
            glib::ControlFlow::Break
        });
    }

    dialog.present();
}

// ---------------------------------------------------------------------------
// Диалог экспорта (0.2): формат, шаблон имени, OCR, подписание, отправка

/// Формат экспорта в порядке комбобокса.
fn export_kind_at(pos: u32) -> crate::app::ExportKind {
    match pos {
        1 => crate::app::ExportKind::Png,
        2 => crate::app::ExportKind::Jpeg,
        3 => crate::app::ExportKind::Tiff,
        _ => crate::app::ExportKind::Pdf,
    }
}

fn export_ext(kind: crate::app::ExportKind) -> &'static str {
    match kind {
        crate::app::ExportKind::Png => "png",
        crate::app::ExportKind::Jpeg => "jpg",
        crate::app::ExportKind::Tiff => "tiff",
        crate::app::ExportKind::Pdf => "pdf",
    }
}

/// Диалог «Экспорт документа».
///
/// * формат PDF/PNG/JPEG/TIFF;
/// * имя файла по шаблону ({date} {time} {profile} {device});
/// * OCR — поисковый PDF через локальный Tesseract (для PDF);
/// * подписание ГОСТ — внешний инструмент из настроек (для PDF);
/// * кнопки «Отправить в папку…» и «Отправить по почте…».
pub fn show_export(ctx: &Rc<AppCtx>, default: crate::app::ExportKind) {
    let lang = ctx.state.borrow().lang;

    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "export.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(520, 560);
    header_close(&header, &dialog, &t(lang, "app.cancel"), false);

    content.set_spacing(10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    // Формат.
    let formats = ["PDF", "PNG", "JPEG", "TIFF"];
    let dd_format = gtk::DropDown::from_strings(&formats);
    dd_format.set_selected(match default {
        crate::app::ExportKind::Png => 1,
        crate::app::ExportKind::Jpeg => 2,
        crate::app::ExportKind::Tiff => 3,
        crate::app::ExportKind::Pdf => 0,
    });
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&t(lang, "export.format")));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        row.append(&dd_format);
        content.append(&row);
    }

    // Имя файла по шаблону.
    let name_entry = gtk::Entry::new();
    {
        let st = ctx.state.borrow();
        let ctx_n = scanner_core::naming::NameContext {
            profile: st
                .profiles
                .first()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| t(lang, "opt.profile.default")),
            device: st
                .device
                .as_ref()
                .map(|d| d.name.clone())
                .unwrap_or_default(),
        };
        name_entry.set_text(&scanner_core::naming::expand_template(
            &st.name_template.clone(),
            &ctx_n,
        ));
    }
    {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let label = gtk::Label::new(Some(&t(lang, "export.filename")));
        label.set_xalign(0.0);
        let hint = gtk::Label::new(Some(&t(lang, "export.template.hint")));
        hint.set_xalign(0.0);
        hint.add_css_class("caption");
        hint.add_css_class("dimmed");
        row.append(&label);
        row.append(&name_entry);
        row.append(&hint);
        content.append(&row);
    }

    // OCR (только для PDF; нужен локальный Tesseract).
    let sw_ocr = adw::SwitchRow::new();
    sw_ocr.set_title(&t(lang, "export.ocr"));
    let ocr_ok = scanner_core::ocr::available();
    if ocr_ok {
        sw_ocr.set_subtitle(
            &scanner_core::ocr::detect().unwrap_or_else(|| "tesseract".into()),
        );
    } else {
        sw_ocr.set_subtitle(&t(lang, "export.ocr.unavailable"));
        sw_ocr.set_sensitive(false);
    }
    // Язык распознавания.
    let dd_ocr_lang = {
        let langs = ["rus+eng", "rus", "eng"];
        let dd = gtk::DropDown::from_strings(&langs);
        let saved = ctx.state.borrow().store.as_ref().and_then(|s| s.get_setting("ocr_lang")).unwrap_or_default();
        dd.set_selected(match saved.as_str() {
            "rus" => 1,
            "eng" => 2,
            _ => 0,
        });
        dd
    };
    let group_ocr = adw::PreferencesGroup::new();
    {
        group_ocr.add(&sw_ocr);
        let row = adw::ActionRow::new();
        row.set_title(&t(lang, "export.ocr.lang"));
        row.add_suffix(&dd_ocr_lang);
        group_ocr.add(&row);
        content.append(&group_ocr);
        group_ocr.set_sensitive(ocr_ok);
    }

    // Подписание ГОСТ (если настроена команда).
    let sw_sign = adw::SwitchRow::new();
    sw_sign.set_title(&t(lang, "export.sign"));
    sw_sign.set_subtitle(&t(lang, "export.sign.subtitle"));
    let group_sign = adw::PreferencesGroup::new();
    {
        let st = ctx.state.borrow();
        sw_sign.set_active(st.sign_enabled && !st.sign_cmd.trim().is_empty());
        sw_sign.set_sensitive(!st.sign_cmd.trim().is_empty());
        group_sign.add(&sw_sign);
        content.append(&group_sign);
        group_sign.set_visible(!st.sign_cmd.trim().is_empty());
    }

    // Кнопки действий.
    let btn_export = gtk::Button::with_label(&t(lang, "export.run"));
    btn_export.add_css_class("suggested-action");
    btn_export.add_css_class("pill");
    let btn_folder = gtk::Button::with_label(&t(lang, "export.to_folder"));
    btn_folder.add_css_class("flat");
    let btn_email = gtk::Button::with_label(&t(lang, "export.to_email"));
    btn_email.add_css_class("flat");
    let btns = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btns.append(&btn_folder);
    btns.append(&btn_email);
    btns.append(&btn_export);
    content.append(&btns);

    // Выбранный формат управляет доступностью OCR/подписи (только PDF).
    {
        let group_ocr = group_ocr.clone();
        let group_sign = group_sign.clone();
        dd_format.connect_selected_notify(move |dd| {
            let is_pdf = dd.selected() == 0;
            group_ocr.set_visible(is_pdf);
            group_sign.set_visible(is_pdf);
        });
    }

    // Экспорт: открыть выбор файла с подставленным именем.
    {
        let ctx = ctx.clone();
        let name_entry = name_entry.clone();
        let dd_format = dd_format.clone();
        let sw_ocr = sw_ocr.clone();
        let dd_ocr_lang = dd_ocr_lang.clone();
        let sw_sign = sw_sign.clone();
        let dialog_c = dialog.clone();
        btn_export.connect_clicked(move |_| {
            let lang = ctx.state.borrow().lang;
            let kind = export_kind_at(dd_format.selected());
            let ext = export_ext(kind);
            let raw_name = name_entry.text().trim().to_string();
            let base = scanner_core::naming::sanitize(&raw_name);
            let save = gtk::FileChooserDialog::new(
                Some(&t(lang, "export.title")),
                Some(&ctx.ui.window),
                gtk::FileChooserAction::Save,
                &[
                    (&t(lang, "app.cancel"), gtk::ResponseType::Cancel),
                    (&t(lang, "app.save"), gtk::ResponseType::Accept),
                ],
            );
            save.set_current_name(&format!("{base}.{ext}"));
            {
                let ctx = ctx.clone();
                let dd_format = dd_format.clone();
                let sw_ocr = sw_ocr.clone();
                let dd_ocr_lang = dd_ocr_lang.clone();
                let sw_sign = sw_sign.clone();
                save.connect_response(move |d, resp| {
                    if resp == gtk::ResponseType::Accept {
                        if let Some(path) = d.file().and_then(|f| f.path()) {
                            let kind = export_kind_at(dd_format.selected());
                            let ext = export_ext(kind);
                            let path = if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                                path
                            } else {
                                path.with_extension(ext)
                            };
                            let ocr = if sw_ocr.is_active() && ocr_ok {
                                let langs = ["rus+eng", "rus", "eng"];
                                let l = langs
                                    .get(dd_ocr_lang.selected() as usize)
                                    .copied()
                                    .unwrap_or("rus+eng");
                                ctx.state.borrow_mut().set_setting("ocr_lang", l);
                                Some(l.to_string())
                            } else {
                                None
                            };
                            let sign = sw_sign.is_active();
                            crate::app::start_export_public(&ctx, kind, path, ocr, sign);
                        }
                    }
                    d.close();
                });
            }
            save.present();
            dialog_c.close();
        });
    }

    // Отправка в папку.
    {
        let ctx = ctx.clone();
        let dialog_c = dialog.clone();
        btn_folder.connect_clicked(move |_| {
            dialog_c.close();
            crate::app::send_to_folder_public(&ctx);
        });
    }
    // Отправка по почте.
    {
        let ctx = ctx.clone();
        let dialog_c = dialog.clone();
        btn_email.connect_clicked(move |_| {
            dialog_c.close();
            crate::app::send_by_email_public(&ctx);
        });
    }

    dialog.present();
}

// ---------------------------------------------------------------------------
// Диалог настроек (0.2): наблюдаемая папка, шаблон, команда подписи

/// Диалог «Настройки».
pub fn show_settings(ctx: &Rc<AppCtx>) {
    let lang = ctx.state.borrow().lang;

    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "settings.dialog.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(560, 560);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_spacing(10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    // --- Наблюдаемая папка ---
    let sw_watched = adw::SwitchRow::new();
    sw_watched.set_title(&t(lang, "settings.watched"));
    sw_watched.set_subtitle(&t(lang, "settings.watched.subtitle"));
    let dir_label = gtk::Label::new(None);
    dir_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    dir_label.add_css_class("caption");
    dir_label.add_css_class("dimmed");
    let btn_pick = gtk::Button::with_label(&t(lang, "settings.watched.choose"));
    btn_pick.add_css_class("flat");
    {
        let st = ctx.state.borrow();
        sw_watched.set_active(st.watched_enabled);
        dir_label.set_text(
            &st.watched_dir
                .as_ref()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|| t(lang, "settings.watched.none")),
        );
    }
    {
        let group = adw::PreferencesGroup::new();
        group.add(&sw_watched);
        let row = adw::ActionRow::new();
        row.set_child(Some(&dir_label));
        row.add_suffix(&btn_pick);
        group.add(&row);
        content.append(&group);
    }

    // Выбор папки.
    {
        let ctx = ctx.clone();
        let dir_label = dir_label.clone();
        btn_pick.connect_clicked(move |_| {
            let lang = ctx.state.borrow().lang;
            let choose = gtk::FileChooserDialog::new(
                Some(&t(lang, "settings.watched.choose")),
                Some(&ctx.ui.window),
                gtk::FileChooserAction::SelectFolder,
                &[
                    (&t(lang, "app.cancel"), gtk::ResponseType::Cancel),
                    (&t(lang, "app.apply"), gtk::ResponseType::Accept),
                ],
            );
            let ctx = ctx.clone();
            let dir_label = dir_label.clone();
            choose.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(path) = d.file().and_then(|f| f.path()) {
                        ctx.state.borrow_mut().watched_dir = Some(path.clone());
                        ctx.state
                            .borrow_mut()
                            .set_setting("watched_dir", &path.to_string_lossy());
                        dir_label.set_text(&path.to_string_lossy());
                    }
                }
                d.close();
            });
            choose.present();
        });
    }
    // Включение/выключение.
    {
        let ctx = ctx.clone();
        sw_watched.connect_active_notify(move |sw| {
            let on = sw.is_active();
            ctx.state.borrow_mut().watched_enabled = on;
            ctx.state.borrow_mut().set_setting("watched_enabled", if on { "1" } else { "0" });
        });
    }

    // --- Шаблон имени файла ---
    let entry_template = gtk::Entry::new();
    {
        let st = ctx.state.borrow();
        entry_template.set_text(&st.name_template);
    }
    {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let label = gtk::Label::new(Some(&t(lang, "settings.template")));
        label.set_xalign(0.0);
        let hint = gtk::Label::new(Some(&t(lang, "export.template.hint")));
        hint.set_xalign(0.0);
        hint.add_css_class("caption");
        hint.add_css_class("dimmed");
        row.append(&label);
        row.append(&entry_template);
        row.append(&hint);
        content.append(&row);
    }
    {
        let ctx = ctx.clone();
        entry_template.connect_changed(move |e| {
            let v = e.text().trim().to_string();
            ctx.state.borrow_mut().name_template = if v.is_empty() {
                scanner_core::naming::DEFAULT_TEMPLATE.into()
            } else {
                v.clone()
            };
            let stored = if v.is_empty() { scanner_core::naming::DEFAULT_TEMPLATE.into() } else { v };
            ctx.state.borrow_mut().set_setting("name_template", &stored);
        });
    }

    // --- Порог пустоты ---
    let spin_threshold = gtk::SpinButton::with_range(0.1, 5.0, 0.1);
    {
        let st = ctx.state.borrow();
        spin_threshold.set_value((st.blank_threshold * 100.0) as f64);
    }
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&t(lang, "settings.threshold")));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        row.append(&spin_threshold);
        content.append(&row);
    }
    {
        let ctx = ctx.clone();
        spin_threshold.connect_value_changed(move |s| {
            let v = (s.value() as f32 / 100.0).clamp(0.001, 0.05);
            ctx.state.borrow_mut().blank_threshold = v;
            ctx.state.borrow_mut().set_setting("blank_threshold", &v.to_string());
        });
    }

    // --- Команда подписи (ГОСТ) ---
    let entry_sign = gtk::Entry::new();
    {
        let st = ctx.state.borrow();
        entry_sign.set_text(&st.sign_cmd);
    }
    entry_sign.set_placeholder_text(Some("cryptcp -signf -detach {file}"));
    let sw_sign_after = adw::SwitchRow::new();
    sw_sign_after.set_title(&t(lang, "settings.sign_after"));
    {
        let st = ctx.state.borrow();
        sw_sign_after.set_active(st.sign_enabled);
    }
    {
        let group = adw::PreferencesGroup::new();
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let label = gtk::Label::new(Some(&t(lang, "settings.sign_cmd")));
        label.set_xalign(0.0);
        let hint = gtk::Label::new(Some(&t(lang, "settings.sign_cmd.hint")));
        hint.set_xalign(0.0);
        hint.add_css_class("caption");
        hint.add_css_class("dimmed");
        row.append(&label);
        row.append(&entry_sign);
        row.append(&hint);
        group.add(&row);
        group.add(&sw_sign_after);
        content.append(&group);
    }
    {
        let ctx = ctx.clone();
        entry_sign.connect_changed(move |e| {
            let v = e.text().trim().to_string();
            ctx.state.borrow_mut().sign_cmd = v.clone();
            ctx.state.borrow_mut().set_setting("sign_cmd", &v);
        });
    }
    {
        let ctx = ctx.clone();
        sw_sign_after.connect_active_notify(move |sw| {
            let on = sw.is_active();
            ctx.state.borrow_mut().sign_enabled = on;
            ctx.state.borrow_mut().set_setting("sign_enabled", if on { "1" } else { "0" });
        });
    }

    // --- Внешние переводы ---
    {
        let group = adw::PreferencesGroup::new();
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let label = gtk::Label::new(Some(&t(lang, "settings.l10n_folder")));
        label.set_xalign(0.0);
        let hint = gtk::Label::new(Some(&t(lang, "settings.l10n.hint")));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("caption");
        hint.add_css_class("dimmed");
        row.append(&label);
        row.append(&hint);
        let btn = gtk::Button::with_label(&t(lang, "settings.l10n_folder"));
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Start);
        {
            btn.connect_clicked(move |_| {
                let dir = scanner_core::i18n::user_l10n_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
            });
        }
        row.append(&btn);
        group.add(&row);
        content.append(&group);
    }

    dialog.present();
}

// ---------------------------------------------------------------------------
// Предпросмотр устройства (низкое DPI, не добавляется в документ)

/// Окно предпросмотра произвольного файла (например, снимка планшета).
pub fn show_device_preview(ctx: &Rc<AppCtx>, path: std::path::PathBuf) {
    let lang = ctx.state.borrow().lang;
    let (dialog, content, header) = dialog_shell();
    dialog.set_title(Some(&t(lang, "preview.title")));
    dialog.set_transient_for(Some(&ctx.ui.window));
    dialog.set_modal(true);
    dialog.set_default_size(720, 640);
    header_close(&header, &dialog, &t(lang, "app.close"), false);

    content.set_spacing(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);

    let pic = gtk::Picture::new();
    pic.set_size_request(680, 540);
    pic.set_can_shrink(true);
    content.append(&pic);

    let hint = gtk::Label::new(Some(&t(lang, "preview.hint")));
    hint.add_css_class("dimmed");
    hint.add_css_class("caption");
    content.append(&hint);

    glib::timeout_add_local(std::time::Duration::from_millis(60), {
        let pic = pic.clone();
        move || {
            if let Some(tex) = crate::state::load_texture(&path) {
                pic.set_paintable(Some(&tex));
            }
            glib::ControlFlow::Break
        }
    });

    dialog.present();
}
