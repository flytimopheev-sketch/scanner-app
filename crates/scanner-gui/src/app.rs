//! Главное окно приложения и вся логика UI.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::gdk;
use gtk4::glib;
use scanner_core::i18n::{t, Lang};
use scanner_core::imageproc::{CropRect, Rotation};
use scanner_core::{
    CapabilitiesResult, DeviceListResult, PageFormat, Request, ScanEvent, ScanMode, ScanOptions,
    ScanProfile, ScanSource, ScannerDevice, TestIpResult,
};

use crate::state::{PageItem, SharedState};
use crate::ui::dialogs::{self, AddDeviceUi};
use crate::worker::{WorkerClient, WorkerIn};

// ---------------------------------------------------------------------------
// Типы

pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub toast: adw::ToastOverlay,
    pub device_combo: gtk::DropDown,
    /// Индикатор состояния выбранного устройства (в шапке).
    pub dev_state_icon: gtk::Image,
    pub status: gtk::Label,
    pub progress: gtk::ProgressBar,
    pub stack: gtk::Stack,
    pub flow: gtk::FlowBox,
    pub btn_scan: gtk::Button,
    pub btn_cancel: gtk::Button,
    pub btn_preview: gtk::Button,
    pub btn_pdf: gtk::Button,
    pub btn_export: gtk::MenuButton,
    pub btn_import: gtk::Button,
    pub page_bar: gtk::Box,
    pub dd_source: gtk::DropDown,
    pub dd_dpi: gtk::DropDown,
    pub dd_mode: gtk::DropDown,
    pub dd_format: gtk::DropDown,
    pub profile_combo: gtk::DropDown,
    /// Ручной дуплекс (два прохода с переворотом стопа).
    pub sw_duplex: adw::SwitchRow,
    /// Пропускать пустые страницы.
    pub sw_blank: adw::SwitchRow,
    /// Кэш эскизов: путь -> текстура.
    pub thumbs: RefCell<HashMap<String, gtk::gdk::Texture>>,
    /// Picture каждой плитки (для обновления эскизов).
    pub pics: RefCell<Vec<gtk::Picture>>,
    /// Плитки (для подсветки выбранной).
    pub tiles: RefCell<Vec<gtk::Box>>,
    /// Подавление обработчиков при программной установке значений.
    pub suppress: Cell<bool>,
    /// Открытый диалог добавления устройства.
    pub add_dialog: RefCell<Option<Rc<AddDeviceUi>>>,
}

pub enum Pending {
    Devices,
    Capabilities,
    TestIp(Rc<AddDeviceUi>),
    /// Предпросмотр планшета в низком разрешении.
    Preview,
}

pub enum UiMsg {
    ThumbReady { idx: usize, path: PathBuf },
    Toast(String),
    Saved(Vec<PathBuf>),
    ExportErr(String),
    /// Результаты авто-фикса: (индекс, угол, рамка обрезки).
    AutoFixed(Vec<(usize, f32, Option<CropRect>)>),
    /// Проверка новой страницы на пустоту завершена.
    PageChecked { page: u32, path: PathBuf, blank: bool },
    /// PDF подписан внешним инструментом (ГОСТ).
    Signed(String),
}

pub struct AppCtx {
    pub state: SharedState,
    pub worker: Rc<WorkerClient>,
    pub pending: Rc<RefCell<HashMap<u64, Pending>>>,
    pub ui: Rc<Ui>,
    pub ui_msg_tx: async_channel::Sender<UiMsg>,
}

pub type Ctx = Rc<AppCtx>;

#[derive(Clone, Copy, PartialEq)]
pub enum ExportKind {
    Png,
    Jpeg,
    Tiff,
    Pdf,
}

// ---------------------------------------------------------------------------
// Точка входа GUI

pub fn run(app: &adw::Application) -> anyhow::Result<()> {
    // Иконки, которых нет в системной теме, регистрируются первым делом —
    // до создания виджетов.
    crate::icons::register();

    let state = Rc::new(RefCell::new(crate::state::AppState::load()));

    // Тема по настройке.
    let theme = state
        .borrow()
        .store
        .as_ref()
        .and_then(|s| s.get_setting("theme"))
        .unwrap_or_else(|| "system".into());
    apply_theme(&theme);

    gtk::Window::set_default_icon_name("scanner-app");

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t(state.borrow().lang, "app.title"))
        .default_width(1180).default_height(780)
        .build();

    let ui = build_main_ui(&window, &state);

    let (ui_msg_tx, ui_msg_rx) = async_channel::unbounded::<UiMsg>();

    // Пульс прогресса во время сканирования.
    {
        let state = state.clone();
        let progress = ui.progress.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            if state.borrow().scanning {
                progress.pulse();
            }
            glib::ControlFlow::Continue
        });
    }

    let pending: Rc<RefCell<HashMap<u64, Pending>>> = Rc::new(RefCell::new(HashMap::new()));

    // Воркер (контекст замыкается позже, см. ctx_holder).
    let ctx_holder: Rc<RefCell<Option<Ctx>>> = Rc::new(RefCell::new(None));
    let worker = {
        let holder = ctx_holder.clone();
        WorkerClient::spawn(move |msg| {
            if let Some(ctx) = holder.borrow().as_ref() {
                handle_worker_msg(ctx, msg);
            }
        })?
    };

    let ctx: Ctx = Rc::new(AppCtx {
        state,
        worker,
        pending,
        ui: ui.clone(),
        ui_msg_tx,
    });
    *ctx_holder.borrow_mut() = Some(ctx.clone());

    install_actions(&ctx);
    startup_state(&ctx);

    // Доставка сообщений фоновых потоков (эскизы, экспорт, авто-фикс).
    {
        let ctx = ctx.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = ui_msg_rx.recv().await {
                handle_ui_msg(&ctx, msg);
            }
        });
    }

    // Наблюдаемая папка: периодическая проверка на новые изображения
    // (МФУ с прямой записью в общую папку). Опрос лёгкий — на главном цикле.
    {
        let ctx = ctx.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(2000), move || {
            poll_watched_folder(&ctx);
            glib::ControlFlow::Continue
        });
    }

    // Показать главное окно (иначе окно создаётся, но не отображается).
    window.present();

    // Режим снимков интерфейса (SCANNER_SHOTS=<каталог>) — no-op в обычной работе.
    crate::shots::maybe_run(&ctx);

    // Периодическая проверка переполнения кэша эскизов не требуется:
    // размер кэша ограничен числом страниц документа.
    Ok(())
}

// ---------------------------------------------------------------------------
// Построение интерфейса

fn build_main_ui(window: &adw::ApplicationWindow, state: &SharedState) -> Rc<Ui> {
    let lang = state.borrow().lang;

    let toast = adw::ToastOverlay::new();

    // --- Header ---
    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new(&t(lang, "app.title"), "");

    let device_combo = gtk::DropDown::from_strings(&[&t(lang, "device.none")]);
    device_combo.set_size_request(260, -1);
    device_combo.set_hexpand(false);

    // Индикатор состояния выбранного устройства (обновляется по ответам).
    let dev_state_icon = gtk::Image::from_icon_name("dialog-question-symbolic");
    dev_state_icon.add_css_class("dev-unknown");
    dev_state_icon.set_tooltip_text(Some(&t(lang, "device.status.unknown")));
    dev_state_icon.set_valign(gtk::Align::Center);

    let btn_add = gtk::Button::from_icon_name("list-add-symbolic");
    btn_add.set_tooltip_text(Some(&t(lang, "device.add")));
    btn_add.set_action_name(Some("win.add-device"));

    let btn_refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    btn_refresh.set_tooltip_text(Some(&t(lang, "device.refresh")));
    btn_refresh.set_action_name(Some("win.refresh-devices"));

    header.pack_start(&btn_refresh);
    header.pack_start(&device_combo);
    header.pack_start(&dev_state_icon);
    header.pack_start(&btn_add);
    header.set_title_widget(Some(&title));

    // Меню настроек.
    let menu = build_menu(state);
    let btn_menu = gtk::MenuButton::new();
    btn_menu.set_icon_name("open-menu-symbolic");
    btn_menu.set_menu_model(Some(&menu));
    header.pack_end(&btn_menu);

    // --- Боковая панель ---
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.add_css_class("sidebar");
    sidebar.set_size_request(292, -1);
    sidebar.set_valign(gtk::Align::Start);

    // Профиль
    let group_profile = adw::PreferencesGroup::new();
    group_profile.set_title(&t(lang, "opt.profile"));
    let profile_row = adw::ActionRow::new();
    profile_row.set_title(&t(lang, "opt.profile"));
    let profile_combo = gtk::DropDown::from_strings(&[&t(lang, "opt.profile.default")]);
    profile_combo.set_size_request(150, -1);
    profile_row.add_suffix(&profile_combo);
    group_profile.add(&profile_row);

    let profile_btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let btn_profile_save = gtk::Button::with_label(&t(lang, "opt.profile.save_current"));
    btn_profile_save.add_css_class("flat");
    btn_profile_save.set_hexpand(true);
    let btn_profile_manage = gtk::Button::from_icon_name("view-list-symbolic");
    btn_profile_manage.set_tooltip_text(Some(&t(lang, "profiles.title")));
    profile_btns.append(&btn_profile_save);
    profile_btns.append(&btn_profile_manage);
    group_profile.add(&profile_btns);
    sidebar.append(&group_profile);

    // Параметры сканирования
    let group_opts = adw::PreferencesGroup::new();
    group_opts.set_title(&t(lang, "opt.source"));

    let mk_row = |title: &str, dd: &gtk::DropDown| {
        let row = adw::ActionRow::new();
        row.set_title(title);
        row.add_suffix(dd);
        row
    };

    let dd_source = gtk::DropDown::from_strings(&[&t(lang, "opt.source.flatbed")]);
    let dd_dpi = gtk::DropDown::from_strings(&["75", "150", "200", "300", "600"]);
    let dd_mode = gtk::DropDown::from_strings(&[&t(lang, "opt.mode.color")]);
    let dd_format = gtk::DropDown::from_strings(&[
        &t(lang, "opt.format.a4"),
        &t(lang, "opt.format.a5"),
        &t(lang, "opt.format.letter"),
        &t(lang, "opt.format.max"),
    ]);

    group_opts.add(&mk_row(&t(lang, "opt.source"), &dd_source));
    group_opts.add(&mk_row(&t(lang, "opt.resolution"), &dd_dpi));
    group_opts.add(&mk_row(&t(lang, "opt.mode"), &dd_mode));
    group_opts.add(&mk_row(&t(lang, "opt.format"), &dd_format));
    sidebar.append(&group_opts);

    // Обработка: ручной дуплекс и пропуск пустых страниц.
    let group_process = adw::PreferencesGroup::new();
    group_process.set_title(&t(lang, "process.group"));
    let sw_duplex = adw::SwitchRow::new();
    sw_duplex.set_title(&t(lang, "duplex.row"));
    sw_duplex.set_subtitle(&t(lang, "duplex.row.subtitle"));
    let sw_blank = adw::SwitchRow::new();
    sw_blank.set_title(&t(lang, "blank.row"));
    sw_blank.set_subtitle(&t(lang, "blank.row.subtitle"));
    group_process.add(&sw_duplex);
    group_process.add(&sw_blank);
    sidebar.append(&group_process);

    // --- Центральная область ---
    let status = gtk::Label::new(None);
    status.add_css_class("status-label");
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let progress = gtk::ProgressBar::new();
    progress.set_size_request(200, -1);
    progress.set_valign(gtk::Align::Center);
    progress.set_visible(false);

    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    status_bar.append(&status);
    status_bar.append(&progress);

    // Пустое состояние
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 0);
    empty.add_css_class("empty-state");
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    let empty_icon = gtk::Image::from_icon_name("scanner-symbolic");
    empty_icon.add_css_class("empty-icon");
    let empty_title = gtk::Label::new(Some(&t(lang, "pages.empty.title")));
    empty_title.add_css_class("empty-title");
    let empty_hint = gtk::Label::new(Some(&t(lang, "pages.empty.hint")));
    empty_hint.add_css_class("empty-hint");
    empty.append(&empty_icon);
    empty.append(&empty_title);
    empty.append(&empty_hint);

    // Галерея страниц
    let flow = gtk::FlowBox::new();
    flow.set_orientation(gtk::Orientation::Horizontal);
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(5);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_margin_top(8);
    flow.set_margin_bottom(8);
    flow.set_margin_start(8);
    flow.set_margin_end(8);
    flow.set_valign(gtk::Align::Start);
    flow.set_selection_mode(gtk::SelectionMode::None);

    let gallery_scroll = gtk::ScrolledWindow::new();
    gallery_scroll.set_child(Some(&flow));
    gallery_scroll.set_vexpand(true);
    gallery_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_titled(&empty, Some("empty"), "empty");
    stack.add_titled(&gallery_scroll, Some("gallery"), "gallery");

    // Панель действий над страницей
    let page_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    page_bar.add_css_class("page-actions");
    page_bar.set_halign(gtk::Align::Center);

    let mk_icon_btn = |icon: &str, tip: &str| {
        let b = gtk::Button::from_icon_name(icon);
        b.set_tooltip_text(Some(tip));
        b.add_css_class("flat");
        b
    };
    let btn_rot_l = mk_icon_btn("object-rotate-left-symbolic", &t(lang, "action.rotate_left"));
    let btn_rot_r = mk_icon_btn("object-rotate-right-symbolic", &t(lang, "action.rotate_right"));
    let btn_crop = mk_icon_btn("tool-crop-symbolic", &t(lang, "action.crop"));
    let btn_enhance = mk_icon_btn("color-select-symbolic", &t(lang, "action.enhance"));
    let btn_autofix = mk_icon_btn("image-auto-adjust-symbolic", &t(lang, "action.autofix"));
    let btn_move_l = mk_icon_btn("go-previous-symbolic", &t(lang, "action.move_left"));
    let btn_move_r = mk_icon_btn("go-next-symbolic", &t(lang, "action.move_right"));
    let btn_del = mk_icon_btn("user-trash-symbolic", &t(lang, "app.delete"));
    btn_del.add_css_class("destructive-action");
    // Кнопки активируют window-действия (одна строка вместо замыканий).
    btn_rot_l.set_action_name(Some("win.rotate-left"));
    btn_rot_r.set_action_name(Some("win.rotate-right"));
    btn_crop.set_action_name(Some("win.crop"));
    btn_enhance.set_action_name(Some("win.enhance"));
    btn_autofix.set_action_name(Some("win.autofix"));
    btn_move_l.set_action_name(Some("win.move-left"));
    btn_move_r.set_action_name(Some("win.move-right"));
    btn_del.set_action_name(Some("win.page-delete"));
    page_bar.append(&btn_rot_l);
    page_bar.append(&btn_rot_r);
    page_bar.append(&btn_crop);
    page_bar.append(&btn_enhance);
    page_bar.append(&btn_autofix);
    page_bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    page_bar.append(&btn_move_l);
    page_bar.append(&btn_move_r);
    page_bar.append(&btn_del);

    // Меню «Ещё»: массовые операции над всеми страницами.
    let more_menu = gio::Menu::new();
    more_menu.append(Some(&t(lang, "action.rotate_all_left")), Some("win.rotate-all-left"));
    more_menu.append(Some(&t(lang, "action.rotate_all_right")), Some("win.rotate-all-right"));
    more_menu.append(Some(&t(lang, "action.enhance_all")), Some("win.enhance-all"));
    more_menu.append(Some(&t(lang, "action.autofix_all")), Some("win.autofix-all"));
    more_menu.append(Some(&t(lang, "action.clear_all")), Some("win.clear-all"));
    let btn_more = gtk::MenuButton::new();
    btn_more.set_icon_name("view-more-symbolic");
    btn_more.set_menu_model(Some(&more_menu));
    btn_more.set_tooltip_text(Some(&t(lang, "action.more")));
    btn_more.add_css_class("flat");
    page_bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    page_bar.append(&btn_more);

    // --- Нижняя панель ---
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.set_margin_top(4);
    bottom.set_margin_bottom(12);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);

    // Кнопка с иконкой и подписью (set_icon_name заменяет подпись).
    let btn_with_icon = |icon: &str, label: &str| -> gtk::Button {
        let b = gtk::Button::new();
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let lbl = gtk::Label::new(Some(label));
        lbl.set_halign(gtk::Align::Center);
        box_.append(&gtk::Image::from_icon_name(icon));
        box_.append(&lbl);
        b.set_child(Some(&box_));
        b
    };

    let btn_import = btn_with_icon("document-open-symbolic", &t(lang, "action.import_files"));

    let btn_preview = gtk::Button::with_label(&t(lang, "action.preview"));

    let spacer1 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer1.set_hexpand(true);

    let btn_scan = btn_with_icon("scanner-symbolic", &t(lang, "action.scan"));
    btn_scan.add_css_class("suggested-action");
    btn_scan.add_css_class("pill");
    btn_scan.set_widget_name("scan-button");

    let btn_cancel = gtk::Button::with_label(&t(lang, "action.cancel"));
    btn_cancel.add_css_class("destructive-action");
    btn_cancel.add_css_class("pill");
    btn_cancel.set_sensitive(false);

    let spacer2 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer2.set_hexpand(true);

    let btn_export = gtk::MenuButton::new();
    btn_export.set_label(&t(lang, "action.export_images"));
    btn_export.add_css_class("flat");
    let export_menu = gio::Menu::new();
    export_menu.append(Some(&t(lang, "export.title")), Some("win.export"));
    export_menu.append(Some("PNG"), Some("win.export-png"));
    export_menu.append(Some("JPEG"), Some("win.export-jpeg"));
    export_menu.append(Some("TIFF"), Some("win.export-tiff"));
    export_menu.append(Some(&t(lang, "export.to_folder")), Some("win.send-folder"));
    export_menu.append(Some(&t(lang, "export.to_email")), Some("win.send-email"));
    btn_export.set_menu_model(Some(&export_menu));

    let btn_pdf = btn_with_icon("document-save-symbolic", &t(lang, "action.save_pdf"));
    btn_pdf.add_css_class("pill");
    btn_pdf.set_widget_name("pdf-button");

    bottom.append(&btn_import);
    bottom.append(&btn_preview);
    bottom.append(&spacer1);
    bottom.append(&btn_scan);
    bottom.append(&btn_cancel);
    bottom.append(&spacer2);
    bottom.append(&btn_export);
    bottom.append(&btn_pdf);

    // --- Сборка ---
    let main_h = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    main_h.set_margin_top(6);
    main_h.set_margin_bottom(4);
    main_h.set_margin_start(12);
    main_h.set_margin_end(12);
    main_h.append(&sidebar);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_vexpand(true);
    content.set_hexpand(true);
    content.append(&status_bar);
    content.append(&stack);
    content.append(&page_bar);
    main_h.append(&content);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    toast.set_child(Some(&main_h));
    root.append(&toast);
    root.append(&bottom);

    window.set_content(Some(&root));

    let ui = Rc::new(Ui {
        window: window.clone(),
        toast,
        device_combo,
        dev_state_icon,
        status,
        progress,
        stack,
        flow,
        btn_scan,
        btn_cancel,
        btn_preview,
        btn_pdf,
        btn_export,
        btn_import,
        page_bar,
        dd_source,
        dd_dpi,
        dd_mode,
        dd_format,
        profile_combo,
        sw_duplex,
        sw_blank,
        thumbs: RefCell::new(HashMap::new()),
        pics: RefCell::new(Vec::new()),
        tiles: RefCell::new(Vec::new()),
        suppress: Cell::new(false),
        add_dialog: RefCell::new(None),
    });

    // Привязка сигналов, зависящих только от виджетов, делается через
    // install_actions (нужен контекст). Здесь — видимые мелочи.
    ui.btn_cancel.set_visible(true);
    ui.page_bar.set_visible(false);
    // Начальные состояния переключателей (сохранённые настройки).
    ui.sw_duplex.set_active(state.borrow().duplex_manual);
    ui.sw_blank.set_active(state.borrow().skip_blank);

    ui
}

fn build_menu(state: &SharedState) -> gio::Menu {
    let lang = state.borrow().lang;

    let section = gio::Menu::new();

    let lang_menu = gio::Menu::new();
    let ru = gio::MenuItem::new(Some(Lang::Ru.display_name()), None);
    ru.set_action_and_target_value(Some("win.lang"), Some(&"ru".to_variant()));
    lang_menu.append_item(&ru);
    let en = gio::MenuItem::new(Some(Lang::En.display_name()), None);
    en.set_action_and_target_value(Some("win.lang"), Some(&"en".to_variant()));
    lang_menu.append_item(&en);

    let theme_menu = gio::Menu::new();
    let mk_theme = |label: &str, value: &str| {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("win.theme"), Some(&value.to_variant()));
        item
    };
    theme_menu.append_item(&mk_theme(&t(lang, "settings.theme.system"), "system"));
    theme_menu.append_item(&mk_theme(&t(lang, "settings.theme.light"), "light"));
    theme_menu.append_item(&mk_theme(&t(lang, "settings.theme.dark"), "dark"));

    let themes = gio::MenuItem::new(Some(&t(lang, "settings.theme")), None);
    themes.set_submenu(Some(&theme_menu));
    let langs = gio::MenuItem::new(Some(&t(lang, "settings.language")), None);
    langs.set_submenu(Some(&lang_menu));

    section.append_item(&langs);
    section.append_item(&themes);

    let actions = gio::Menu::new();
    actions.append(Some(&t(lang, "settings.dialog.title")), Some("win.settings"));
    actions.append(Some(&t(lang, "settings.l10n_folder")), Some("win.l10n-folder"));
    actions.append(Some(&t(lang, "settings.log")), Some("win.log"));
    actions.append(Some(&t(lang, "settings.about")), Some("win.about"));

    let menu = gio::Menu::new();
    menu.append_section(None, &section);
    menu.append_section(None, &actions);
    menu
}

// ---------------------------------------------------------------------------
// Действия

fn install_actions(ctx: &Ctx) {
    let window = ctx.ui.window.clone();
    let act = |name: &str| gio::SimpleAction::new(name, None);

    macro_rules! bind {
        ($name:literal, $f:expr) => {{
            let a = act($name);
            let ctx = ctx.clone();
            a.connect_activate(move |_, _| {
                let f: &dyn Fn(&Ctx) = &$f;
                f(&ctx);
            });
            window.add_action(&a);
        }};
    }

    bind!("scan", do_scan);
    bind!("cancel", do_cancel);
    bind!("refresh-devices", refresh_devices);
    bind!("add-device", open_add_device);
    bind!("import", open_import);
    bind!("preview", do_preview);
    bind!("save-pdf", |ctx| open_export_dialog(ctx, ExportKind::Pdf));
    bind!("export", |ctx| open_export_dialog(ctx, ExportKind::Pdf));
    bind!("export-png", |ctx| open_export_dialog(ctx, ExportKind::Png));
    bind!("export-jpeg", |ctx| open_export_dialog(ctx, ExportKind::Jpeg));
    bind!("export-tiff", |ctx| open_export_dialog(ctx, ExportKind::Tiff));
    bind!("send-folder", send_to_folder);
    bind!("send-email", send_by_email);
    bind!("settings", |ctx| dialogs::show_settings(ctx));
    bind!("l10n-folder", open_l10n_folder);
    bind!("rotate-left", |ctx| rotate_page(ctx, false));
    bind!("rotate-right", |ctx| rotate_page(ctx, true));
    bind!("crop", |ctx| dialogs::show_crop(ctx));
    bind!("enhance", toggle_enhance);
    bind!("autofix", autofix_selected);
    bind!("move-left", |ctx| move_page(ctx, -1));
    bind!("move-right", |ctx| move_page(ctx, 1));
    bind!("page-delete", delete_page);
    bind!("clear-all", clear_pages);
    bind!("rotate-all-left", |ctx| rotate_all(ctx, false));
    bind!("rotate-all-right", |ctx| rotate_all(ctx, true));
    bind!("enhance-all", enhance_all);
    bind!("autofix-all", autofix_all);
    bind!("profile-save", |ctx| dialogs::show_save_profile(ctx));
    bind!("profile-manage", |ctx| dialogs::show_profiles(ctx));
    bind!("log", |ctx| dialogs::show_log(ctx));

    // Открытие папки из toast после экспорта (xdg-open).
    let a_open = gio::SimpleAction::new("open-path", Some(glib::VariantTy::STRING));
    a_open.connect_activate(|_, param| {
        if let Some(path) = param.and_then(|v| v.str()) {
            let _ = std::process::Command::new("xdg-open").arg(path).spawn();
        }
    });
    window.add_action(&a_open);

    // Горячие клавиши.
    if let Some(app) = window
        .application()
        .and_then(|a| a.downcast::<gtk::Application>().ok())
    {
        app.set_accels_for_action("win.import", &["<Primary>O"]);
        app.set_accels_for_action("win.save-pdf", &["<Primary>S"]);
        app.set_accels_for_action("win.scan", &["<Primary>Return"]);
        app.set_accels_for_action("win.preview", &["<Primary>P"]);
        app.set_accels_for_action("win.refresh-devices", &["F5"]);
        app.set_accels_for_action("win.page-delete", &["Delete"]);
        app.set_accels_for_action("win.rotate-right", &["<Primary>R"]);
        app.set_accels_for_action("win.rotate-left", &["<Primary><Shift>R"]);
        app.set_accels_for_action("win.clear-all", &["<Primary>Delete"]);
    }

    // Перетаскивание файлов изображений в окно (drag & drop).
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    {
        let ctx = ctx.clone();
        drop.connect_drop(move |_, value, _, _| {
            let lang = ctx.state.borrow().lang;
            if ctx.state.borrow().scanning {
                return false;
            }
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let mut added = 0usize;
            {
                let mut st = ctx.state.borrow_mut();
                for f in list.files() {
                    let Some(p) = f.path() else { continue };
                    let ext = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase());
                    if matches!(
                        ext.as_deref(),
                        Some("png" | "jpg" | "jpeg" | "tif" | "tiff")
                    ) {
                        st.pages.push(PageItem {
                            source: p,
                            edit: Default::default(),
                        });
                        added += 1;
                    }
                }
                if added > 0 {
                    st.selected = Some(st.pages.len() - 1);
                }
            }
            if added > 0 {
                rebuild_gallery(&ctx);
                toast(&ctx, &scanner_core::tf!(lang, "dnd.added", added));
            } else {
                toast(&ctx, &t(lang, "dnd.unsupported"));
            }
            true
        });
    }
    window.add_controller(drop);

    // Язык (состояние для галочки в меню).
    let lang_value = ctx.state.borrow().lang.code().to_string();
    let a_lang =
        gio::SimpleAction::new_stateful("lang", Some(glib::VariantTy::STRING), &lang_value.to_variant());
    {
        let ctx = ctx.clone();
        a_lang.connect_activate(move |a, param| {
            let code = param
                .and_then(|v| v.str().map(|s| s.to_string()))
                .unwrap_or_else(|| "ru".into());
            let lang = Lang::from_code(&code);
            a.set_state(&code.to_variant());
            ctx.state.borrow_mut().set_setting("lang", code.as_str());
            ctx.ui
                .toast
                .add_toast(adw::Toast::new(&t(lang, "lang.changed")));
        });
    }
    window.add_action(&a_lang);

    // Тема.
    let theme_value = ctx
        .state
        .borrow()
        .store
        .as_ref()
        .and_then(|s| s.get_setting("theme"))
        .unwrap_or_else(|| "system".into());
    let a_theme = gio::SimpleAction::new_stateful(
        "theme",
        Some(glib::VariantTy::STRING),
        &theme_value.to_variant(),
    );
    {
        let ctx = ctx.clone();
        a_theme.connect_activate(move |a, param| {
            let value = param
                .and_then(|v| v.str().map(|s| s.to_string()))
                .unwrap_or_else(|| "system".into());
            a.set_state(&value.to_variant());
            apply_theme(&value);
            ctx.state.borrow_mut().set_setting("theme", value.as_str());
            ctx.ui
                .toast
                .add_toast(adw::Toast::new(&t(ctx.state.borrow().lang, "theme.changed")));
        });
    }
    window.add_action(&a_theme);

    // О программе.
    {
        let ctx = ctx.clone();
        let a_about = act("about");
        a_about.connect_activate(move |_, _| {
            let lang = ctx.state.borrow().lang;
            let about = gtk::AboutDialog::new();
            about.set_program_name(Some(&t(lang, "app.title")));
            // Уникальный заголовок окна (режим снимков ищет окно по title).
            about.set_title(Some(&t(lang, "settings.about")));
            about.set_version(Some(env!("CARGO_PKG_VERSION")));
            about.set_comments(Some(&t(lang, "about.comments")));
            about.set_license_type(gtk::License::Gpl30);
            about.set_logo_icon_name(Some("scanner-app"));
            about.set_transient_for(Some(&ctx.ui.window));
            about.set_modal(true);
            about.present();
        });
        window.add_action(&a_about);
    }

    // Переключатели обработки: ручной дуплекс и пропуск пустых страниц.
    {
        let ctx = ctx.clone();
        let sw = ctx.ui.sw_duplex.clone();
        sw.connect_active_notify(move |sw| {
            let on = sw.is_active();
            ctx.state.borrow_mut().duplex_manual = on;
            ctx.state.borrow_mut().set_setting("duplex_manual", if on { "1" } else { "0" });
        });
    }
    {
        let ctx = ctx.clone();
        let sw = ctx.ui.sw_blank.clone();
        sw.connect_active_notify(move |sw| {
            let on = sw.is_active();
            ctx.state.borrow_mut().skip_blank = on;
            ctx.state.borrow_mut().set_setting("skip_blank", if on { "1" } else { "0" });
        });
    }

    // Изменения значений в комбобоксах.
    bind_option_changed(ctx);
}

fn bind_option_changed(ctx: &Ctx) {
    // Источник
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.dd_source.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let list = source_list(&ctx.state.borrow());
            if let Some(src) = list.get(dd.selected() as usize) {
                ctx.state.borrow_mut().options.source = *src;
                save_options(&ctx);
            }
        });
    }
    // DPI
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.dd_dpi.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let list = dpi_list(&ctx.state.borrow());
            if let Some(dpi) = list.get(dd.selected() as usize) {
                ctx.state.borrow_mut().options.resolution = *dpi;
                save_options(&ctx);
            }
        });
    }
    // Режим
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.dd_mode.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let list = mode_list(&ctx.state.borrow());
            if let Some(mode) = list.get(dd.selected() as usize) {
                ctx.state.borrow_mut().options.mode = *mode;
                save_options(&ctx);
            }
        });
    }
    // Формат
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.dd_format.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let list = PageFormat::all();
            if let Some(fmt) = list.get(dd.selected() as usize) {
                ctx.state.borrow_mut().options.format = *fmt;
                save_options(&ctx);
            }
        });
    }
    // Профиль
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.profile_combo.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let sel = dd.selected() as usize;
            let profiles = ctx.state.borrow().profiles.clone();
            if sel >= 1 {
                if let Some(p) = profiles.get(sel - 1) {
                    apply_profile(&ctx, p.clone());
                }
            }
        });
    }
    // Устройство
    {
        let ctx = ctx.clone();
        let dd = ctx.ui.device_combo.clone();
        dd.connect_selected_notify(move |dd| {
            if ctx.ui.suppress.get() {
                return;
            }
            let devices = ctx.state.borrow().devices.clone();
            if let Some(dev) = devices.get(dd.selected() as usize) {
                select_device(&ctx, dev.clone());
            }
        });
    }
}

fn apply_theme(mode: &str) {
    let scheme = match mode {
        "light" => adw::ColorScheme::PreferLight,
        "dark" => adw::ColorScheme::PreferDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

// ---------------------------------------------------------------------------
// Списки значений выпадающих списков

fn dpi_list(state: &std::cell::Ref<crate::state::AppState>) -> Vec<u32> {
    state
        .caps
        .as_ref()
        .map(|c| c.resolutions.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![75, 150, 200, 300, 600])
}

fn mode_list(state: &std::cell::Ref<crate::state::AppState>) -> Vec<ScanMode> {
    state
        .caps
        .as_ref()
        .map(|c| c.modes.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| ScanMode::all().to_vec())
}

fn source_list(state: &std::cell::Ref<crate::state::AppState>) -> Vec<ScanSource> {
    state
        .caps
        .as_ref()
        .map(|c| c.sources.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| ScanSource::all().to_vec())
}

fn refresh_option_dropdowns(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let state = ctx.state.borrow();
    let opts = state.options;
    ctx.ui.suppress.set(true);

    // Источник
    let sources = source_list(&state);
    let items: Vec<String> = sources
        .iter()
        .map(|s| match s {
            ScanSource::Flatbed => t(lang, "opt.source.flatbed"),
            ScanSource::Adf => t(lang, "opt.source.adf"),
            ScanSource::AdfDuplex => t(lang, "opt.source.adf_duplex"),
        })
        .collect();
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    ctx.ui.dd_source.set_model(Some(&gtk::StringList::new(&refs)));
    let sel = sources.iter().position(|s| *s == opts.source).unwrap_or(0);
    ctx.ui.dd_source.set_selected(sel as u32);

    // DPI
    let dpis = dpi_list(&state);
    let items: Vec<String> = dpis.iter().map(|d| d.to_string()).collect();
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    ctx.ui.dd_dpi.set_model(Some(&gtk::StringList::new(&refs)));
    let sel = dpis
        .iter()
        .enumerate()
        .min_by_key(|(_, d)| (**d as i32 - opts.resolution as i32).abs())
        .map(|(i, _)| i)
        .unwrap_or(3);
    ctx.ui.dd_dpi.set_selected(sel as u32);

    // Режим
    let modes = mode_list(&state);
    let items: Vec<String> = modes
        .iter()
        .map(|m| match m {
            ScanMode::Color => t(lang, "opt.mode.color"),
            ScanMode::Gray => t(lang, "opt.mode.gray"),
            ScanMode::Lineart => t(lang, "opt.mode.lineart"),
        })
        .collect();
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    ctx.ui.dd_mode.set_model(Some(&gtk::StringList::new(&refs)));
    let sel = modes.iter().position(|m| *m == opts.mode).unwrap_or(0);
    ctx.ui.dd_mode.set_selected(sel as u32);

    ctx.ui.suppress.set(false);
}

fn save_options(ctx: &Ctx) {
    let json = serde_json::to_string(&ctx.state.borrow().options).unwrap_or_default();
    ctx.state.borrow_mut().set_setting("last_options", &json);
}

// ---------------------------------------------------------------------------
// Устройства

fn rebuild_device_combo(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let state = ctx.state.borrow();
    ctx.ui.suppress.set(true);

    let items: Vec<String> = if state.devices.is_empty() {
        vec![t(lang, "device.not_found")]
    } else {
        state
            .devices
            .iter()
            .map(|d| {
                let kind = match d.kind {
                    scanner_core::DeviceKind::Usb => t(lang, "device.kind.usb"),
                    scanner_core::DeviceKind::Network => t(lang, "device.kind.network"),
                    scanner_core::DeviceKind::Unknown => t(lang, "device.unknown"),
                };
                format!("{} · {}", d.name, kind)
            })
            .collect()
    };
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    ctx.ui.device_combo.set_model(Some(&gtk::StringList::new(&refs)));

    // Фабрика строк: индикатор состояния + подпись. Список пунктов
    // соответствует state.devices, поэтому позиция = устройство.
    let factory = gtk::SignalListItemFactory::new();
    {
        factory.connect_setup(move |_f, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let dot = gtk::Image::from_icon_name("media-record-symbolic");
            dot.add_css_class("dev-dot");
            dot.add_css_class("dev-unknown");
            let label = gtk::Label::new(None);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&dot);
            row.append(&label);
            item.set_child(Some(&row));
        });
    }
    {
        let ctx = ctx.clone();
        factory.connect_bind(move |_f, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let pos = item.position();
            let Some(row) = item.child().and_then(|c| c.downcast::<gtk::Box>().ok()) else {
                return;
            };
            let Some(dot) = row.first_child().and_then(|c| c.downcast::<gtk::Image>().ok())
            else {
                return;
            };
            for cls in ["dev-ok", "dev-down", "dev-unknown"] {
                dot.remove_css_class(cls);
            }
            let (name, status, tip) = {
                let st = ctx.state.borrow();
                match st.devices.get(pos as usize) {
                    Some(dev) => {
                        let stt = st
                            .dev_status
                            .get(&dev.device_name)
                            .copied()
                            .unwrap_or(crate::state::DevStatus::Unknown);
                        let tip = match stt {
                            crate::state::DevStatus::Ok => t(st.lang, "device.status.ok"),
                            crate::state::DevStatus::Down => t(st.lang, "device.status.down"),
                            crate::state::DevStatus::Unknown => {
                                t(st.lang, "device.status.unknown")
                            }
                        };
                        (dev.name.clone(), stt, tip)
                    }
                    None => {
                        let s = t(st.lang, "device.not_found");
                        let tip = s.clone();
                        (s, crate::state::DevStatus::Unknown, tip)
                    }
                }
            };
            match status {
                crate::state::DevStatus::Ok => dot.add_css_class("dev-ok"),
                crate::state::DevStatus::Down => dot.add_css_class("dev-down"),
                crate::state::DevStatus::Unknown => dot.add_css_class("dev-unknown"),
            }
            dot.set_tooltip_text(Some(&tip));
            if let Some(label) = row.last_child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                label.set_text(&name);
            }
        });
    }
    ctx.ui.device_combo.set_factory(Some(&factory));

    let sel = state
        .device
        .as_ref()
        .and_then(|dev| state.devices.iter().position(|d| d.device_name == dev.device_name))
        .map(|i| i as u32)
        .unwrap_or(0);
    ctx.ui.device_combo.set_selected(sel);
    ctx.ui.suppress.set(false);
}

/// Обновить индикатор состояния устройства (шапка + статус в state).
fn set_device_status(ctx: &Ctx, device_name: &str, status: crate::state::DevStatus) {
    ctx.state
        .borrow_mut()
        .dev_status
        .insert(device_name.to_string(), status);
    // Индикатор выбранного устройства обновляем только если это оно.
    let is_selected = ctx
        .state
        .borrow()
        .device
        .as_ref()
        .map(|d| d.device_name == device_name)
        .unwrap_or(false);
    if !is_selected {
        return;
    }
    let lang = ctx.state.borrow().lang;
    let icon = ctx.ui.dev_state_icon.clone();
    for cls in ["dev-ok", "dev-down", "dev-unknown"] {
        icon.remove_css_class(cls);
    }
    match status {
        crate::state::DevStatus::Ok => {
            icon.add_css_class("dev-ok");
            icon.set_icon_name(Some("object-select-symbolic"));
            icon.set_tooltip_text(Some(&t(lang, "device.status.ok")));
        }
        crate::state::DevStatus::Down => {
            icon.add_css_class("dev-down");
            icon.set_icon_name(Some("network-offline-symbolic"));
            icon.set_tooltip_text(Some(&t(lang, "device.status.down")));
        }
        crate::state::DevStatus::Unknown => {
            icon.add_css_class("dev-unknown");
            icon.set_icon_name(Some("dialog-question-symbolic"));
            icon.set_tooltip_text(Some(&t(lang, "device.status.unknown")));
        }
    }
    // Перерисовать фабрику комбобокса (цвета точек).
    ctx.ui.device_combo.queue_draw();
}

pub fn select_device(ctx: &Ctx, dev: ScannerDevice) {
    {
        let mut st = ctx.state.borrow_mut();
        st.device = Some(dev.clone());
        st.caps = None;
        st.set_setting("device", &dev.device_name);
    }
    rebuild_device_combo(ctx);
    set_status(ctx, &dev.name, false);
    let id = ctx.worker.request(Request::Capabilities {
        device: dev.device_name.clone(),
    });
    ctx.pending.borrow_mut().insert(id, Pending::Capabilities);
}

pub fn add_manual_device(ctx: &Ctx, dev: ScannerDevice) {
    {
        let mut st = ctx.state.borrow_mut();
        if !st.devices.iter().any(|d| d.device_name == dev.device_name) {
            st.devices.push(dev.clone());
        }
    }
    select_device(ctx, dev);
}

fn fill_found_list(ctx: &Ctx, ad: &AddDeviceUi) {
    let lang = ctx.state.borrow().lang;
    while let Some(child) = ad.found_list.first_child() {
        ad.found_list.remove(&child);
    }
    let devices = ctx.state.borrow().devices.clone();
    for dev in devices {
        let kind = match dev.kind {
            scanner_core::DeviceKind::Usb => t(lang, "device.kind.usb"),
            scanner_core::DeviceKind::Network => t(lang, "device.kind.network"),
            scanner_core::DeviceKind::Unknown => t(lang, "device.unknown"),
        };
        let row_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let name = gtk::Label::new(Some(&dev.name));
        name.set_xalign(0.0);
        name.add_css_class("heading");
        let sub = gtk::Label::new(Some(&format!("{kind} · {}", dev.device_name)));
        sub.set_xalign(0.0);
        sub.add_css_class("caption");
        sub.add_css_class("dimmed");
        row_box.append(&name);
        row_box.append(&sub);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_selectable(true);
        ad.found_list.append(&row);
    }
}

pub fn refresh_devices(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    set_status(ctx, &t(lang, "device.searching"), false);
    let id = ctx.worker.request(Request::ListDevices);
    ctx.pending.borrow_mut().insert(id, Pending::Devices);
}

fn merge_devices(ctx: &Ctx, found: Vec<ScannerDevice>) {
    let lang = ctx.state.borrow().lang;
    let saved = ctx.state.borrow().saved_devices();
    let mut all = found;
    for s in saved {
        if !all.iter().any(|d| d.device_name == s.device_name) {
            all.push(s);
        }
    }
    {
        let mut st = ctx.state.borrow_mut();
        st.devices = all;
        // Выбираем прежнее устройство, если оно ещё есть.
        if st.device.is_none() {
            if let Some(name) = st.last_device_name.clone() {
                st.device = st.devices.iter().find(|d| d.device_name == name).cloned();
            }
            if st.device.is_none() {
                st.device = st.devices.first().cloned();
            }
        }
    }
    rebuild_device_combo(ctx);

    let status_text = {
        let st = ctx.state.borrow();
        match &st.device {
            Some(d) => d.name.clone(),
            None => t(lang, "device.not_found"),
        }
    };
    set_status(ctx, &status_text, false);

    // Запросить возможности выбранного.
    if let Some(dev) = ctx.state.borrow().device.clone() {
        let id = ctx
            .worker
            .request(Request::Capabilities { device: dev.device_name });
        ctx.pending.borrow_mut().insert(id, Pending::Capabilities);
    }

    if let Some(ad) = ctx.ui.add_dialog.borrow().as_ref() {
        fill_found_list(ctx, ad);
    }
}

// ---------------------------------------------------------------------------
// Сканирование

pub fn do_scan(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let dev = match ctx.state.borrow().device.clone() {
        Some(d) => d,
        None => {
            toast(ctx, &t(lang, "status.no_device"));
            return;
        }
    };
    let options = ctx.state.borrow().options;
    let out_dir = scanner_core::config::tmp_scan_dir()
        .join(format!("job-{}", chrono::Local::now().format("%Y%m%d-%H%M%S%3f")));

    {
        let mut st = ctx.state.borrow_mut();
        // Ручной дуплекс: если это не аппаратный дуплекс и мы не в проходе 2.
        if st.duplex_manual
            && options.source != ScanSource::AdfDuplex
            && st.duplex_pass == crate::state::DuplexPass::None
        {
            st.duplex_pass = crate::state::DuplexPass::Front;
            st.duplex_front_start = st.pages.len();
            st.duplex_front_count = 0;
        }
        st.scanning = true;
    }
    set_scanning_ui(ctx, true);
    let status_text = if ctx.state.borrow().duplex_pass == crate::state::DuplexPass::Back {
        t(lang, "duplex.pass2")
    } else {
        t(lang, "status.scanning")
    };
    set_status(ctx, &status_text, false);

    ctx.worker.request(Request::Scan {
        device: dev.device_name,
        options,
        out_dir: out_dir.to_string_lossy().into_owned(),
    });
}

/// Предпросмотр: если есть устройство — низкое DPI с планшета (страница
/// НЕ попадает в документ), иначе — просмотр выбранной страницы.
fn do_preview(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let dev = ctx.state.borrow().device.clone();
    match dev {
        Some(dev) if !ctx.state.borrow().scanning => {
            ctx.state.borrow_mut().previewing = true;
            let out_dir = scanner_core::config::tmp_scan_dir()
                .join(format!("preview-{}", chrono::Local::now().format("%Y%m%d-%H%M%S%3f")));
            set_status(ctx, &t(lang, "preview.title"), false);
            let id = ctx.worker.request(Request::Preview {
                device: dev.device_name,
                dpi: 75,
                out_dir: out_dir.to_string_lossy().into_owned(),
            });
            ctx.pending.borrow_mut().insert(id, Pending::Preview);
        }
        _ => dialogs::show_preview(ctx),
    }
}

/// Диалог «Переверните стоп» между проходами ручного дуплекса.
fn show_flip_dialog(ctx: &Ctx, front_count: usize) {
    let lang = ctx.state.borrow().lang;
    let text = scanner_core::tf!(lang, "duplex.flip.text", front_count);
    let dialog = adw::MessageDialog::builder()
        .heading(&t(lang, "duplex.flip.title"))
        .body(&text)
        .transient_for(&ctx.ui.window)
        .modal(true)
        .build();
    dialog.add_response("cancel", &t(lang, "app.cancel"));
    dialog.set_response_appearance("cancel", adw::ResponseAppearance::Destructive);
    dialog.add_response("continue", &t(lang, "duplex.flip.continue"));
    dialog.set_response_appearance("continue", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("continue"));
    dialog.set_close_response("cancel");

    {
        let ctx = ctx.clone();
        dialog.connect_response(None, move |_d, response| {
            let lang = ctx.state.borrow().lang;
            if response == "continue" {
                // Второй проход: сканируем оборотные стороны.
                ctx.state.borrow_mut().duplex_pass = crate::state::DuplexPass::Back;
                do_scan(&ctx);
            } else {
                // Отмена дуплекса: страницы остаются как есть (лицевые).
                ctx.state.borrow_mut().duplex_pass = crate::state::DuplexPass::None;
                set_status(&ctx, &t(lang, "status.scan_done"), false);
            }
        });
    }
    dialog.present();
}

/// Переплести лицевые и оборотные страницы после второго прохода дуплекса.
fn finish_duplex(ctx: &Ctx) {
    let (front_start, front_count, total) = {
        let st = ctx.state.borrow();
        (st.duplex_front_start, st.duplex_front_count, st.pages.len())
    };
    let front: Vec<crate::state::PageItem> =
        ctx.state.borrow().pages[front_start..front_start + front_count].to_vec();
    let back: Vec<crate::state::PageItem> = ctx.state.borrow().pages[front_start + front_count..]
        .to_vec();
    let merged = scanner_core::duplex::interleave_duplex(&front, &back);
    let count = merged.len();
    {
        let mut st = ctx.state.borrow_mut();
        st.pages.splice(
            front_start..total,
            merged,
        );
        st.duplex_pass = crate::state::DuplexPass::None;
        st.selected = Some(front_start.min(st.pages.len().saturating_sub(1)));
    }
    rebuild_gallery(ctx);
    let lang = ctx.state.borrow().lang;
    toast(ctx, &scanner_core::tf!(lang, "duplex.done", count));
    set_status(
        ctx,
        &scanner_core::tf!(lang, "pages.count", ctx.state.borrow().pages.len()),
        false,
    );
}

pub fn do_cancel(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    set_status(ctx, &t(lang, "status.cancelling"), false);
    ctx.worker.request(Request::Cancel);
}

fn set_scanning_ui(ctx: &Ctx, scanning: bool) {
    let ui = &ctx.ui;
    ui.btn_scan.set_sensitive(!scanning);
    ui.btn_cancel.set_sensitive(scanning);
    ui.progress.set_visible(scanning);
    ui.progress.set_fraction(0.0);
    ui.device_combo.set_sensitive(!scanning);
    ui.dd_source.set_sensitive(!scanning);
    ui.dd_dpi.set_sensitive(!scanning);
    ui.dd_mode.set_sensitive(!scanning);
    ui.dd_format.set_sensitive(!scanning);
    ui.btn_import.set_sensitive(!scanning);
    ui.btn_export.set_sensitive(!scanning);
    ui.btn_pdf.set_sensitive(!scanning);
    ui.btn_preview.set_sensitive(!scanning);
    ui.page_bar.set_sensitive(!scanning);
}

// ---------------------------------------------------------------------------
// Страницы

fn rebuild_gallery(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let ui = &ctx.ui;
    let state = ctx.state.borrow();

    ui.suppress.set(true);
    ui.pics.borrow_mut().clear();
    ui.tiles.borrow_mut().clear();
    while let Some(child) = ui.flow.first_child() {
        ui.flow.remove(&child);
    }

    for (idx, page) in state.pages.iter().enumerate() {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
        tile.add_css_class("page-tile");
        if state.selected == Some(idx) {
            tile.add_css_class("page-tile-selected");
        }

        let pic = gtk::Picture::new();
        pic.set_size_request(184, 244);
        pic.set_halign(gtk::Align::Center);
        tile.append(&pic);
        ui.pics.borrow_mut().push(pic.clone());

        let label = gtk::Label::new(Some(&scanner_core::tf!(lang, "page.n", idx + 1)));
        label.add_css_class("page-number");
        tile.append(&label);

        {
            let ctx = ctx.clone();
            let idx = idx;
            let gesture = gtk::GestureClick::new();
            gesture.connect_pressed(move |g, _, _, _| {
                g.set_state(gtk::EventSequenceState::Claimed);
                select_page(&ctx, idx);
            });
            tile.add_controller(gesture);
        }

        // Перетаскивание плитки для смены порядка страниц (drag & drop).
        {
            let src = gtk::DragSource::new();
            src.set_actions(gdk::DragAction::MOVE);
            src.connect_prepare(move |_, _, _| {
                let value = (idx as u32).to_value();
                Some(gdk::ContentProvider::for_value(&value))
            });
            tile.add_controller(src);

            let ctx = ctx.clone();
            let target_idx = idx;
            let dst = gtk::DropTarget::new(glib::types::Type::U32, gdk::DragAction::MOVE);
            dst.connect_drop(move |_, value, _, _| {
                let Ok(from) = value.get::<u32>() else {
                    return false;
                };
                reorder_pages(&ctx, from as usize, target_idx);
                true
            });
            tile.add_controller(dst);
        }

        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&tile));
        child.set_can_focus(false);
        child.set_focusable(false);
        ui.flow.append(&child);
        ui.tiles.borrow_mut().push(tile);

        // Эскиз
        let thumb_path = crate::state::edited_cache_path(&page.source, &page.edit, "thumb");
        let key = thumb_path.to_string_lossy().into_owned();
        let cached = ui.thumbs.borrow().get(&key).cloned();
        if let Some(tex) = cached {
            pic.set_paintable(Some(&tex));
        } else if thumb_path.exists() {
            if let Some(tex) = crate::state::load_texture(&thumb_path) {
                ui.thumbs.borrow_mut().insert(key, tex.clone());
                pic.set_paintable(Some(&tex));
            }
        } else {
            let tx = ctx.ui_msg_tx.clone();
            let src = page.source.clone();
            let edit = page.edit;
            let out = thumb_path;
            std::thread::spawn(move || {
                if crate::state::render_thumb(&src, &edit, &out).is_ok() {
                    tx.send_blocking(UiMsg::ThumbReady { idx, path: out }).ok();
                }
            });
        }
    }

    // Плитка-заглушка во время сканирования: пульсирующий спиннер.
    if state.scanning {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 8);
        tile.add_css_class("page-tile");
        tile.add_css_class("page-tile-scanning");
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(184, 200);
        spinner.set_valign(gtk::Align::Center);
        spinner.set_halign(gtk::Align::Center);
        spinner.start();
        tile.append(&spinner);
        let label = gtk::Label::new(Some(&t(lang, "status.scanning")));
        label.add_css_class("page-number");
        tile.append(&label);
        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&tile));
        child.set_can_focus(false);
        child.set_focusable(false);
        ui.flow.append(&child);
    }

    ui.suppress.set(false);
    update_stack(ctx, state.pages.len());
}

fn update_stack(ctx: &Ctx, page_count: usize) {
    let lang = ctx.state.borrow().lang;
    if page_count == 0 {
        ctx.ui.stack.set_visible_child_name("empty");
        ctx.ui.page_bar.set_visible(false);
        if !ctx.state.borrow().scanning {
            set_status(ctx, &t(lang, "pages.empty.hint"), false);
        }
    } else {
        ctx.ui.stack.set_visible_child_name("gallery");
        ctx.ui.page_bar.set_visible(true);
        if !ctx.state.borrow().scanning {
            set_status(
                ctx,
                &scanner_core::tf!(lang, "pages.count", page_count),
                false,
            );
        }
    }
}

pub fn select_page(ctx: &Ctx, idx: usize) {
    ctx.state.borrow_mut().selected = Some(idx);
    let tiles = ctx.ui.tiles.borrow();
    for (i, tile) in tiles.iter().enumerate() {
        if i == idx {
            tile.add_css_class("page-tile-selected");
        } else {
            tile.remove_css_class("page-tile-selected");
        }
    }
}

pub fn set_page_crop(ctx: &Ctx, idx: usize, crop: Option<CropRect>) {
    {
        let mut st = ctx.state.borrow_mut();
        if let Some(page) = st.pages.get_mut(idx) {
            page.edit.crop = crop;
        }
    }
    rebuild_gallery(ctx);
}

fn rotate_page(ctx: &Ctx, clockwise: bool) {
    {
        let mut st = ctx.state.borrow_mut();
        if let Some(sel) = st.selected {
            if let Some(page) = st.pages.get_mut(sel) {
                page.edit.rotation = if clockwise {
                    page.edit.rotation.next_cw()
                } else {
                    page.edit.rotation.next_ccw()
                };
            }
        }
    }
    rebuild_gallery(ctx);
}

fn toggle_enhance(ctx: &Ctx) {
    {
        let mut st = ctx.state.borrow_mut();
        if let Some(sel) = st.selected {
            if let Some(page) = st.pages.get_mut(sel) {
                page.edit.enhance = !page.edit.enhance;
            }
        }
    }
    rebuild_gallery(ctx);
}

// ---------------------------------------------------------------------------
// Авто-выправление (deskew + авто-обрезка)

fn autofix_selected(ctx: &Ctx) {
    let sel = ctx.state.borrow().selected;
    match sel {
        Some(i) => autofix_pages(ctx, vec![i]),
        None => {
            let lang = ctx.state.borrow().lang;
            toast(ctx, &t(lang, "autofix.nothing"));
        }
    }
}

fn autofix_all(ctx: &Ctx) {
    let indices: Vec<usize> = (0..ctx.state.borrow().pages.len()).collect();
    if indices.is_empty() {
        return;
    }
    autofix_pages(ctx, indices);
}

/// Вычислить угол/рамку для списка страниц в фоновом потоке.
fn autofix_pages(ctx: &Ctx, indices: Vec<usize>) {
    let jobs: Vec<(usize, PathBuf, Rotation)> = {
        let st = ctx.state.borrow();
        indices
            .into_iter()
            .filter_map(|i| {
                st.pages
                    .get(i)
                    .map(|p| (i, p.source.clone(), p.edit.rotation))
            })
            .collect()
    };
    if jobs.is_empty() {
        return;
    }
    let lang = ctx.state.borrow().lang;
    set_status(ctx, &t(lang, "autofix.running"), false);

    let tx = ctx.ui_msg_tx.clone();
    std::thread::spawn(move || {
        let mut results = Vec::with_capacity(jobs.len());
        for (idx, src, rot) in jobs {
            if let Ok((angle, crop)) = crate::state::compute_autofix(&src, rot) {
                results.push((idx, angle, crop));
            }
        }
        tx.send_blocking(UiMsg::AutoFixed(results)).ok();
    });
}

fn rotate_all(ctx: &Ctx, clockwise: bool) {
    {
        let mut st = ctx.state.borrow_mut();
        for page in st.pages.iter_mut() {
            page.edit.rotation = if clockwise {
                page.edit.rotation.next_cw()
            } else {
                page.edit.rotation.next_ccw()
            };
        }
    }
    rebuild_gallery(ctx);
}

fn enhance_all(ctx: &Ctx) {
    {
        let mut st = ctx.state.borrow_mut();
        // Включаем всем, если хотя бы у одной выключен; иначе выключаем всем.
        let turn_on = st.pages.iter().any(|p| !p.edit.enhance);
        for page in st.pages.iter_mut() {
            page.edit.enhance = turn_on;
        }
    }
    rebuild_gallery(ctx);
}

fn move_page(ctx: &Ctx, delta: i32) {
    {
        let mut st = ctx.state.borrow_mut();
        let Some(sel) = st.selected else { return };
        let new = sel as i64 + delta as i64;
        if new < 0 || new as usize >= st.pages.len() {
            return;
        }
        let new = new as usize;
        st.pages.swap(sel, new);
        st.selected = Some(new);
    }
    rebuild_gallery(ctx);
}

/// Перестановка страницы drag & drop: from -> to.
fn reorder_pages(ctx: &Ctx, from: usize, to: usize) {
    if from == to {
        return;
    }
    {
        let mut st = ctx.state.borrow_mut();
        if from >= st.pages.len() || to >= st.pages.len() {
            return;
        }
        let page = st.pages.remove(from);
        st.pages.insert(to, page);
        st.selected = Some(to);
    }
    rebuild_gallery(ctx);
}

fn delete_page(ctx: &Ctx) {
    {
        let mut st = ctx.state.borrow_mut();
        let Some(sel) = st.selected else { return };
        if sel < st.pages.len() {
            st.pages.remove(sel);
        }
        st.selected = if st.pages.is_empty() {
            None
        } else {
            Some(sel.min(st.pages.len() - 1))
        };
    }
    rebuild_gallery(ctx);
}

fn clear_pages(ctx: &Ctx) {
    {
        let mut st = ctx.state.borrow_mut();
        st.pages.clear();
        st.selected = None;
    }
    rebuild_gallery(ctx);
}

// ---------------------------------------------------------------------------
// Наблюдаемая папка

/// Опрос наблюдаемой папки: новые изображения автоматически попадают
/// в документ. Первый опрос после включения фиксирует уже лежащие файлы
/// как «исходные» (без импорта), дальше импортируются только новые.
fn poll_watched_folder(ctx: &Ctx) {
    let (enabled, dir, scanning, first_poll) = {
        let st = ctx.state.borrow();
        (
            st.watched_enabled,
            st.watched_dir.clone(),
            st.scanning,
            st.watched_seen.is_empty(),
        )
    };
    if !enabled || scanning {
        return;
    }
    let Some(dir) = dir else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut added = 0usize;
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());
        if !matches!(
            ext.as_deref(),
            Some("png" | "jpg" | "jpeg" | "tif" | "tiff")
        ) {
            continue;
        }
        {
            let mut st = ctx.state.borrow_mut();
            if st.watched_seen.contains(&p) {
                continue;
            }
            st.watched_seen.insert(p.clone());
            if !first_poll {
                st.pages.push(PageItem {
                    source: p,
                    edit: Default::default(),
                });
                added += 1;
            }
        }
    }
    if added > 0 {
        {
            let mut st = ctx.state.borrow_mut();
            st.selected = Some(st.pages.len().saturating_sub(1));
        }
        rebuild_gallery(ctx);
        let lang = ctx.state.borrow().lang;
        toast(ctx, &scanner_core::tf!(lang, "watched.added", added));
    }
}

// ---------------------------------------------------------------------------
// Импорт / экспорт

fn open_import(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let dialog = gtk::FileChooserDialog::new(
        Some(&t(lang, "action.import_files")),
        Some(&ctx.ui.window),
        gtk::FileChooserAction::Open,
        &[
            (&t(lang, "app.cancel"), gtk::ResponseType::Cancel),
            (&t(lang, "app.apply"), gtk::ResponseType::Accept),
        ],
    );
    dialog.set_select_multiple(true);

    let filter = gtk::FileFilter::new();
    filter.set_name(Some("PNG / JPEG / TIFF"));
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.add_mime_type("image/tiff");
    dialog.add_filter(&filter);

    let ctx = ctx.clone();
    dialog.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Accept {
            let mut paths: Vec<PathBuf> = Vec::new();
            let model = d.files();
            for i in 0..model.n_items() {
                if let Some(file) = model.item(i).and_then(|o| o.downcast::<gio::File>().ok()) {
                    if let Some(path) = file.path() {
                        paths.push(path);
                    }
                }
            }
            paths.sort();
            let added = paths.len();
            {
                let mut st = ctx.state.borrow_mut();
                for p in paths {
                    st.pages.push(PageItem { source: p, edit: Default::default() });
                }
                if added > 0 {
                    st.selected = Some(st.pages.len() - 1);
                }
            }
            if added > 0 {
                rebuild_gallery(&ctx);
            }
        }
        d.close();
    });
    dialog.present();
}

/// Выбор каталога наблюдаемой папки / папки отправки.
fn choose_folder(ctx: &Ctx, title: &str, on_pick: impl Fn(&Ctx, PathBuf) + 'static) {
    let lang = ctx.state.borrow().lang;
    let dialog = gtk::FileChooserDialog::new(
        Some(title),
        Some(&ctx.ui.window),
        gtk::FileChooserAction::SelectFolder,
        &[
            (&t(lang, "app.cancel"), gtk::ResponseType::Cancel),
            (&t(lang, "app.apply"), gtk::ResponseType::Accept),
        ],
    );
    let ctx = ctx.clone();
    dialog.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Accept {
            if let Some(path) = d.file().and_then(|f| f.path()) {
                on_pick(&ctx, path);
            }
        }
        d.close();
    });
    dialog.present();
}

/// Диалог экспорта: формат, шаблон имени, OCR, подписание, отправка.
pub fn open_export_dialog(ctx: &Ctx, default: ExportKind) {
    let lang = ctx.state.borrow().lang;
    if ctx.state.borrow().pages.is_empty() {
        toast(ctx, &t(lang, "status.no_pages"));
        return;
    }
    dialogs::show_export(ctx, default);
}

// Публичные обёртки для вызовов из ui::dialogs (модуль dialogs — брат app).

/// Экспорт с опциями OCR/подписи — точка входа из диалога экспорта.
pub fn start_export_public(
    ctx: &Ctx,
    kind: ExportKind,
    target: PathBuf,
    ocr_langs: Option<String>,
    sign: bool,
) {
    start_export_with_opts(ctx, kind, target, ocr_langs, sign);
}

/// Отправка в папку — точка входа из диалога экспорта.
pub fn send_to_folder_public(ctx: &Ctx) {
    send_to_folder(ctx);
}

/// Отправка по почте — точка входа из диалога экспорта.
pub fn send_by_email_public(ctx: &Ctx) {
    send_by_email(ctx);
}

/// Запуск экспорта с учётом OCR и подписания (фоновый поток).
fn start_export_with_opts(
    ctx: &Ctx,
    kind: ExportKind,
    target: PathBuf,
    ocr_langs: Option<String>,
    sign: bool,
) {
    let jobs: Vec<(PathBuf, scanner_core::imageproc::PageEdit)> = {
        let st = ctx.state.borrow();
        st.pages
            .iter()
            .map(|p| (p.source.clone(), p.edit))
            .collect()
    };
    let dpi = ctx.state.borrow().options.resolution.max(150);
    let sign_cmd = if sign {
        ctx.state.borrow().sign_cmd.clone()
    } else {
        String::new()
    };
    let tx = ctx.ui_msg_tx.clone();
    let lang = ctx.state.borrow().lang;

    set_status(ctx, &t(lang, "app.ok"), false);

    std::thread::spawn(move || {
        let result = (|| -> std::result::Result<Vec<PathBuf>, String> {
            let mut edited = Vec::with_capacity(jobs.len());
            for (src, edit) in &jobs {
                let cache = crate::state::edited_cache_path(src, edit, "exp");
                let path = crate::state::render_edited(src, edit, &cache)
                    .map_err(|e| e.to_string())?;
                edited.push(path);
            }
            let base = target.with_extension("");
            match kind {
                ExportKind::Pdf => {
                    // OCR: поисковый PDF через Tesseract (если просили и есть).
                    if let Some(langs) = &ocr_langs {
                        if scanner_core::ocr::available() {
                            let tmp = target
                                .parent()
                                .unwrap_or(std::path::Path::new("."))
                                .join(format!(
                                    ".ocr-{}",
                                    chrono::Local::now().format("%Y%m%d%H%M%S%3f")
                                ));
                            std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
                            let mut page_pdfs = Vec::with_capacity(edited.len());
                            let mut txt_files = Vec::new();
                            for (i, page) in edited.iter().enumerate() {
                                let out_base =
                                    tmp.join(format!("page-{:04}", i + 1));
                                let (pdf, txt) = scanner_core::ocr::ocr_page(
                                    page,
                                    &out_base,
                                    langs,
                                    true,
                                    true,
                                )
                                .map_err(|e| e.to_string())?;
                                if let Some(pdf) = pdf {
                                    page_pdfs.push(pdf);
                                }
                                if let Some(txt) = txt {
                                    txt_files.push(txt);
                                }
                            }
                            if !page_pdfs.is_empty() {
                                scanner_core::export::merge_pdfs(&page_pdfs, &target)
                                    .map_err(|e| e.to_string())?;
                            } else {
                                return Err("tesseract не создал поисковых страниц".into());
                            }
                            // Текстовый слой рядом с PDF (для поиска/индексации).
                            if let Some(dir) = target.parent() {
                                let stem = target
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("scan");
                                let mut all_text = String::new();
                                for txt in &txt_files {
                                    if let Ok(s) = std::fs::read_to_string(txt) {
                                        all_text.push_str(&s);
                                        all_text.push('\n');
                                    }
                                }
                                let _ = std::fs::write(
                                    dir.join(format!("{stem}.txt")),
                                    all_text,
                                );
                            }
                            let _ = std::fs::remove_dir_all(&tmp);
                            return finish_sign(sign_cmd, vec![target], &tx);
                        }
                        return Err(t(
                            scanner_core::i18n::Lang::Ru,
                            "export.ocr.unavailable",
                        ));
                    }
                    scanner_core::export::export_pdf(&edited, &target, dpi, 88)
                        .map_err(|e| e.to_string())?;
                    return finish_sign(sign_cmd, vec![target], &tx);
                }
                ExportKind::Png => {
                    scanner_core::export::export_png(&edited, &base).map_err(|e| e.to_string())
                }
                ExportKind::Jpeg => {
                    scanner_core::export::export_jpeg(&edited, &base, 90)
                        .map_err(|e| e.to_string())
                }
                ExportKind::Tiff => {
                    scanner_core::export::export_tiff(&edited, &base).map_err(|e| e.to_string())
                }
            }
        })();
        match result {
            Ok(files) => tx.send_blocking(UiMsg::Saved(files)).ok(),
            Err(e) => tx.send_blocking(UiMsg::ExportErr(e)).ok(),
        };
    });

    // Подписание выполняется после экспорта (для PDF).
    fn finish_sign(
        sign_cmd: String,
        files: Vec<PathBuf>,
        tx: &async_channel::Sender<UiMsg>,
    ) -> std::result::Result<Vec<PathBuf>, String> {
        if sign_cmd.trim().is_empty() {
            return Ok(files);
        }
        let pdf = files
            .iter()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"))
            .cloned();
        match pdf {
            Some(pdf) => match scanner_core::sign::sign_pdf(&sign_cmd, &pdf) {
                Ok(outcome) => {
                    if let Some(signed) = outcome.output {
                        let _ = tx.send_blocking(UiMsg::Signed(
                            signed.to_string_lossy().into_owned(),
                        ));
                    }
                    Ok(files)
                }
                Err(e) => Err(e.to_string()),
            },
            None => Ok(files),
        }
    }
}

/// Экспорт текущего документа во временную папку (для отправки).
fn export_to_tmp(
    ctx: &Ctx,
    kind: ExportKind,
) -> std::thread::JoinHandle<std::result::Result<Vec<PathBuf>, String>> {
    let jobs: Vec<(PathBuf, scanner_core::imageproc::PageEdit)> = {
        let st = ctx.state.borrow();
        st.pages
            .iter()
            .map(|p| (p.source.clone(), p.edit))
            .collect()
    };
    let dpi = ctx.state.borrow().options.resolution.max(150);
    std::thread::spawn(move || {
        let mut edited = Vec::with_capacity(jobs.len());
        for (src, edit) in &jobs {
            let cache = crate::state::edited_cache_path(src, edit, "exp");
            let path = crate::state::render_edited(src, edit, &cache).map_err(|e| e.to_string())?;
            edited.push(path);
        }
        let tmp = scanner_core::config::tmp_scan_dir().join(format!(
            "send-{}",
            chrono::Local::now().format("%Y%m%d%H%M%S%3f")
        ));
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let base = tmp.join("scan");
        match kind {
            ExportKind::Pdf => {
                let target = base.with_extension("pdf");
                scanner_core::export::export_pdf(&edited, &target, dpi, 88)
                    .map_err(|e| e.to_string())?;
                Ok(vec![target])
            }
            ExportKind::Png => scanner_core::export::export_png(&edited, &base)
                .map_err(|e| e.to_string()),
            ExportKind::Jpeg => scanner_core::export::export_jpeg(&edited, &base, 90)
                .map_err(|e| e.to_string()),
            ExportKind::Tiff => scanner_core::export::export_tiff(&edited, &base)
                .map_err(|e| e.to_string()),
        }
    })
}

/// «Отправить в папку…»: экспорт и копирование в выбранную папку
/// (в том числе примонтированную SMB/ shares).
fn send_to_folder(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    if ctx.state.borrow().pages.is_empty() {
        toast(ctx, &t(lang, "status.no_pages"));
        return;
    }
    let ctx2 = ctx.clone();
    choose_folder(ctx, &t(lang, "export.to_folder"), move |ctx, dir| {
        set_status(ctx, "…", false);
        let ctx3 = ctx2.clone();
        let handle = export_to_tmp(ctx, ExportKind::Pdf);
        let tx = ctx3.ui_msg_tx.clone();
        std::thread::spawn(move || {
            let result = handle
                .join()
                .unwrap_or_else(|_| Err("export thread panicked".into()));
            match result {
                Ok(files) => {
                    let mut copied = 0usize;
                    let mut last_err: Option<String> = None;
                    for f in &files {
                        let dest = dir.join(f.file_name().unwrap_or_default());
                        match std::fs::copy(f, &dest) {
                            Ok(_) => copied += 1,
                            Err(e) => last_err = Some(e.to_string()),
                        }
                    }
                    let lang = scanner_core::i18n::Lang::Ru;
                    if copied > 0 {
                        let _ = tx.send_blocking(UiMsg::Toast(scanner_core::tf!(
                            lang,
                            "export.copied",
                            dir.display()
                        )));
                    } else {
                        let _ = tx.send_blocking(UiMsg::ExportErr(
                            last_err.unwrap_or_else(|| "copy failed".into()),
                        ));
                    }
                }
                Err(e) => {
                    let _ = tx.send_blocking(UiMsg::ExportErr(e));
                }
            }
        });
    });
}

/// «Отправить по почте…»: экспорт PDF и открытие почтового клиента
/// через xdg-email (--attach), если он установлен.
fn send_by_email(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    if ctx.state.borrow().pages.is_empty() {
        toast(ctx, &t(lang, "status.no_pages"));
        return;
    }
    set_status(ctx, "…", false);
    let handle = export_to_tmp(ctx, ExportKind::Pdf);
    let tx = ctx.ui_msg_tx.clone();
    let lang_c = ctx.state.borrow().lang;
    std::thread::spawn(move || {
        let result = handle
            .join()
            .unwrap_or_else(|_| Err("export thread panicked".into()));
        match result {
            Ok(files) => {
                let pdf = files
                    .iter()
                    .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"));
                let Some(pdf) = pdf else {
                    let _ = tx.send_blocking(UiMsg::ExportErr("no pdf".into()));
                    return;
                };
                let res = std::process::Command::new("xdg-email")
                    .args([
                        "--attach",
                        &pdf.to_string_lossy(),
                        "--subject",
                        "Документы",
                    ])
                    .spawn();
                match res {
                    Ok(_) => {
                        let _ = tx.send_blocking(UiMsg::Toast(t(lang_c, "export.email.opened")));
                    }
                    Err(_) => {
                        let _ = tx.send_blocking(UiMsg::Toast(t(
                            lang_c,
                            "export.email.unavailable",
                        )));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send_blocking(UiMsg::ExportErr(e));
            }
        }
    });
}

/// Открыть папку пользовательских переводов (внешняя L10N).
fn open_l10n_folder(ctx: &Ctx) {
    let dir = scanner_core::i18n::user_l10n_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    toast(ctx, &dir.to_string_lossy());
}

// ---------------------------------------------------------------------------
// Профили

pub fn save_profile_named(ctx: &Ctx, name: &str) {
    let lang = ctx.state.borrow().lang;
    let profile = ScanProfile {
        id: None,
        name: name.to_string(),
        device_name: ctx.state.borrow().device.as_ref().map(|d| d.device_name.clone()),
        options: ctx.state.borrow().options,
        duplex_manual: ctx.state.borrow().duplex_manual,
        skip_blank: ctx.state.borrow().skip_blank,
        blank_threshold: ctx.state.borrow().blank_threshold,
    };
    if let Some(store) = &ctx.state.borrow().store {
        let _ = store.save_profile(&profile);
    }
    ctx.state.borrow_mut().reload_profiles();
    refresh_profiles(ctx);
    toast(ctx, &t(lang, "profiles.saved"));
}

fn apply_profile(ctx: &Ctx, profile: ScanProfile) {
    {
        let mut st = ctx.state.borrow_mut();
        st.options = profile.options;
        // Профиль запоминает и режимы обработки.
        st.duplex_manual = profile.duplex_manual;
        st.skip_blank = profile.skip_blank;
        st.blank_threshold = profile.blank_threshold;
        st.set_setting(
            "last_options",
            &serde_json::to_string(&profile.options).unwrap_or_default(),
        );
        st.set_setting("duplex_manual", if profile.duplex_manual { "1" } else { "0" });
        st.set_setting("skip_blank", if profile.skip_blank { "1" } else { "0" });
        st.set_setting("blank_threshold", &profile.blank_threshold.to_string());
    }
    // Переключатели боковой панели — без сохранения в подавленном режиме.
    ctx.ui.suppress.set(true);
    ctx.ui.sw_duplex.set_active(profile.duplex_manual);
    ctx.ui.sw_blank.set_active(profile.skip_blank);
    ctx.ui.suppress.set(false);
    refresh_option_dropdowns(ctx);
    // Привязанное устройство — выбрать, если найдено.
    if let Some(name) = profile.device_name {
        let dev = ctx.state.borrow().devices.iter().find(|d| d.device_name == name).cloned();
        if let Some(dev) = dev {
            select_device(ctx, dev);
        }
    }
    let lang = ctx.state.borrow().lang;
    toast(ctx, &t(lang, "profiles.applied"));
}

pub fn refresh_profiles(ctx: &Ctx) {
    let lang = ctx.state.borrow().lang;
    let state = ctx.state.borrow();
    ctx.ui.suppress.set(true);
    let mut items = vec![t(lang, "opt.profile.default")];
    items.extend(state.profiles.iter().map(|p| p.name.clone()));
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    ctx.ui
        .profile_combo
        .set_model(Some(&gtk::StringList::new(&refs)));
    ctx.ui.profile_combo.set_selected(0);
    ctx.ui.suppress.set(false);
}

// ---------------------------------------------------------------------------
// Открытие диалогов

pub fn open_add_device(ctx: &Ctx) {
    let ad = dialogs::show_add_device(ctx);
    fill_found_list(ctx, &ad);
    *ctx.ui.add_dialog.borrow_mut() = Some(ad);
}

// ---------------------------------------------------------------------------
// Обработка сообщений

pub fn handle_worker_msg(ctx: &Ctx, msg: WorkerIn) {
    match msg {
        WorkerIn::Ok { id, result } => {
            let pending = ctx.pending.borrow_mut().remove(&id);
            match pending {
                Some(Pending::Devices) => {
                    match serde_json::from_value::<DeviceListResult>(result) {
                        Ok(list) => merge_devices(ctx, list.devices),
                        Err(e) => log::warn!("devices parse: {e}"),
                    }
                }
                Some(Pending::Capabilities) => {
                    match serde_json::from_value::<CapabilitiesResult>(result) {
                        Ok(caps) => {
                            ctx.state.borrow_mut().caps = Some(caps.capabilities);
                            let selected = ctx.state.borrow().device.clone();
                            if let Some(dev) = selected {
                                set_device_status(ctx, &dev.device_name, crate::state::DevStatus::Ok);
                            }
                            refresh_option_dropdowns(ctx);
                        }
                        Err(e) => log::warn!("caps parse: {e}"),
                    }
                }
                Some(Pending::Preview) => {
                    ctx.state.borrow_mut().previewing = false;
                    match serde_json::from_value::<scanner_core::PreviewResult>(result) {
                        Ok(prev) => {
                            dialogs::show_device_preview(ctx, PathBuf::from(prev.path));
                        }
                        Err(e) => log::warn!("preview parse: {e}"),
                    }
                }
                Some(Pending::TestIp(ad)) => {
                    ad.spinner.stop();
                    ad.check_btn.set_sensitive(true);
                    match serde_json::from_value::<TestIpResult>(result) {
                        Ok(res) => {
                            ad.result_label.set_text(&res.test.message);
                            if res.test.ok {
                                ad.result_label.remove_css_class("status-error");
                                ad.save_btn.set_sensitive(true);
                            } else {
                                ad.result_label.add_css_class("status-error");
                                ad.save_btn.set_sensitive(false);
                            }
                        }
                        Err(e) => {
                            ad.result_label.add_css_class("status-error");
                            ad.result_label.set_text(&e.to_string());
                            ad.save_btn.set_sensitive(false);
                        }
                    }
                }
                None => {}
            }
        }
        WorkerIn::Err { id, code, message } => {
            let pending = ctx.pending.borrow_mut().remove(&id);
            let text = format!("{code}: {message}");
            match pending {
                Some(Pending::TestIp(ad)) => {
                    ad.spinner.stop();
                    ad.check_btn.set_sensitive(true);
                    ad.result_label.add_css_class("status-error");
                    ad.result_label.set_text(&message);
                    ad.save_btn.set_sensitive(false);
                }
                Some(Pending::Preview) => {
                    ctx.state.borrow_mut().previewing = false;
                    let lang = ctx.state.borrow().lang;
                    toast(ctx, &t(lang, "preview.failed"));
                    scanner_core::config::append_log(&format!("preview: {text}"));
                    // Предпросмотр не удался — устройство помечаем недоступным.
                    let selected = ctx.state.borrow().device.clone();
                    if let Some(dev) = selected {
                        set_device_status(ctx, &dev.device_name, crate::state::DevStatus::Down);
                    }
                }
                Some(Pending::Capabilities) => {
                    // Возможности не получены — устройство недоступно.
                    let selected = ctx.state.borrow().device.clone();
                    if let Some(dev) = selected {
                        set_device_status(ctx, &dev.device_name, crate::state::DevStatus::Down);
                    }
                    set_status(ctx, &text, true);
                    scanner_core::config::append_log(&text);
                }
                Some(Pending::Devices) | None => {
                    set_status(ctx, &text, true);
                    scanner_core::config::append_log(&text);
                }
            }
        }
        WorkerIn::Event(ev) => handle_scan_event(ctx, ev),
        WorkerIn::Dead(reason) => {
            ctx.state.borrow_mut().scanning = false;
            set_scanning_ui(ctx, false);
            let text = format!("scanner-worker: {reason}");
            set_status(ctx, &text, true);
            scanner_core::config::append_log(&text);
        }
    }
}

fn handle_scan_event(ctx: &Ctx, ev: ScanEvent) {
    let lang = ctx.state.borrow().lang;
    match ev {
        ScanEvent::Started => {
            let status_text = if ctx.state.borrow().duplex_pass == crate::state::DuplexPass::Back {
                t(lang, "duplex.pass2")
            } else {
                t(lang, "status.scanning")
            };
            set_status(ctx, &status_text, false)
        }
        ScanEvent::PageStarted { page } => {
            set_status(ctx, &scanner_core::tf!(lang, "status.page", page), false)
        }
        ScanEvent::PageProgress { percent, .. } => {
            // Реальный прогресс страницы (если драйвер его отдаёт).
            ctx.ui
                .progress
                .set_fraction((percent as f64 / 100.0).clamp(0.0, 1.0));
        }
        ScanEvent::PageDone { page, path } => {
            let path = PathBuf::from(&path);
            let skip_blank = ctx.state.borrow().skip_blank;
            let threshold = ctx.state.borrow().blank_threshold;
            if skip_blank {
                // Проверка на пустоту в фоне: страница попадёт в документ
                // только если на ней есть чернила.
                let tx = ctx.ui_msg_tx.clone();
                std::thread::spawn(move || {
                    let blank = scanner_core::imageproc::is_blank_file(&path, threshold)
                        .unwrap_or(false);
                    tx.send_blocking(UiMsg::PageChecked { page, path, blank }).ok();
                });
            } else {
                add_scanned_page(ctx, page, path);
            }
        }
        ScanEvent::Finished { pages, .. } => {
            ctx.state.borrow_mut().scanning = false;
            set_scanning_ui(ctx, false);
            match ctx.state.borrow().duplex_pass {
                crate::state::DuplexPass::Front => {
                    // Лицевые готовы: запоминаем их количество и ждём переворота.
                    let n = pages as usize;
                    ctx.state.borrow_mut().duplex_front_count = n;
                    ctx.state.borrow_mut().duplex_pass = crate::state::DuplexPass::AwaitingFlip;
                    show_flip_dialog(ctx, n);
                }
                crate::state::DuplexPass::Back => finish_duplex(ctx),
                _ => {
                    set_status(
                        ctx,
                        &scanner_core::tf!(lang, "status.scan_done", pages),
                        false,
                    );
                }
            }
        }
        ScanEvent::Cancelled => {
            ctx.state.borrow_mut().scanning = false;
            ctx.state.borrow_mut().duplex_pass = crate::state::DuplexPass::None;
            set_scanning_ui(ctx, false);
            set_status(ctx, &t(lang, "status.scan_cancelled"), false);
        }
        ScanEvent::Failed { error, .. } => {
            ctx.state.borrow_mut().scanning = false;
            ctx.state.borrow_mut().duplex_pass = crate::state::DuplexPass::None;
            set_scanning_ui(ctx, false);
            // Устройство, не отдавшее скан, помечаем недоступным.
            let selected = ctx.state.borrow().device.clone();
            if let Some(dev) = selected {
                set_device_status(ctx, &dev.device_name, crate::state::DevStatus::Down);
            }
            set_status(ctx, &format!("{}: {error}", t(lang, "status.scan_failed")), true);
            scanner_core::config::append_log(&format!("scan failed: {error}"));
        }
    }
}

/// Добавить отсканированную страницу в документ и подсветить плитку.
fn add_scanned_page(ctx: &Ctx, page: u32, path: PathBuf) {
    {
        let mut st = ctx.state.borrow_mut();
        st.pages.push(PageItem {
            source: path,
            edit: Default::default(),
        });
        st.selected = Some(st.pages.len() - 1);
        if st.duplex_pass == crate::state::DuplexPass::Front {
            st.duplex_front_count += 1;
        }
    }
    rebuild_gallery(ctx);
    let lang = ctx.state.borrow().lang;
    set_status(ctx, &scanner_core::tf!(lang, "status.page", page), false);
    // Анимация «новая страница»: подсветка плитки на секунду.
    highlight_last_tile(ctx);
}

fn highlight_last_tile(ctx: &Ctx) {
    let tile = ctx.ui.tiles.borrow().last().cloned();
    if let Some(tile) = tile {
        tile.add_css_class("page-tile-new");
        glib::timeout_add_local_once(std::time::Duration::from_millis(1000), move || {
            tile.remove_css_class("page-tile-new");
        });
    }
}

fn handle_ui_msg(ctx: &Ctx, msg: UiMsg) {
    let ui = &ctx.ui;
    match msg {
        UiMsg::ThumbReady { idx, path } => {
            if let Some(tex) = crate::state::load_texture(&path) {
                let key = path.to_string_lossy().into_owned();
                ui.thumbs.borrow_mut().insert(key, tex.clone());
                let pics = ui.pics.borrow();
                if let Some(pic) = pics.get(idx) {
                    pic.set_paintable(Some(&tex));
                }
            }
        }
        UiMsg::Toast(text) => {
            ui.toast.add_toast(adw::Toast::new(&text));
        }
        UiMsg::Saved(files) => {
            let lang = ctx.state.borrow().lang;
            let names: Vec<String> = files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let toast = adw::Toast::new(&names.join("\n"));
            // Кнопка «Открыть папку» рядом с уведомлением об успехе.
            if let Some(dir) = files.first().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
                toast.set_button_label(Some(&t(lang, "toast.open_folder")));
                toast.set_action_name(Some("win.open-path"));
                toast.set_action_target_value(Some(&dir.to_string_lossy().to_variant()));
                toast.set_timeout(12);
            }
            ui.toast.add_toast(toast);
        }
        UiMsg::ExportErr(e) => {
            ui.toast.add_toast(adw::Toast::new(&e));
            scanner_core::config::append_log(&e);
        }
        UiMsg::AutoFixed(results) => {
            let lang = ctx.state.borrow().lang;
            let n = results.len();
            for (idx, angle, crop) in &results {
                log::info!("autofix page {}: angle={angle}, crop={crop:?}", idx + 1);
            }
            {
                let mut st = ctx.state.borrow_mut();
                for (idx, angle, crop) in results {
                    if let Some(page) = st.pages.get_mut(idx) {
                        page.edit.deskew = angle;
                        page.edit.crop = crop;
                    }
                }
            }
            rebuild_gallery(ctx);
            toast(ctx, &scanner_core::tf!(lang, "autofix.done", n));
        }
        UiMsg::PageChecked { page, path, blank } => {
            if blank {
                // Пустую страницу в документ не добавляем.
                let lang = ctx.state.borrow().lang;
                toast(ctx, &scanner_core::tf!(lang, "blank.skipped", page));
            } else {
                add_scanned_page(ctx, page, path);
            }
        }
        UiMsg::Signed(path) => {
            let lang = ctx.state.borrow().lang;
            toast(ctx, &scanner_core::tf!(lang, "export.signed", path));
        }
    }
}

// ---------------------------------------------------------------------------
// Вспомогательные

fn set_status(ctx: &Ctx, text: &str, is_error: bool) {
    ctx.ui.status.set_text(text);
    if is_error {
        ctx.ui.status.add_css_class("status-error");
    } else {
        ctx.ui.status.remove_css_class("status-error");
    }
}

fn toast(ctx: &Ctx, text: &str) {
    ctx.ui.toast.add_toast(adw::Toast::new(text));
}

/// Начальное состояние: восстановить настройки, устройства, профили.
fn startup_state(ctx: &Ctx) {
    // Последние параметры сканирования.
    // ВАЖНО: сначала извлекаем значение, чтобы временный borrow() из
    // условия if let не жил до конца блока (иначе borrow_mut() ниже паникует).
    let last_options = ctx
        .state
        .borrow()
        .store
        .as_ref()
        .and_then(|s| s.get_setting("last_options"));
    if let Some(json) = last_options {
        if let Ok(opts) = serde_json::from_str::<ScanOptions>(&json) {
            ctx.state.borrow_mut().options = opts;
        }
    }

    // Сохранённые вручную устройства сразу доступны.
    let saved = ctx.state.borrow().saved_devices();
    if !saved.is_empty() {
        let mut st = ctx.state.borrow_mut();
        st.devices = saved;
        if let Some(name) = st.last_device_name.clone() {
            st.device = st.devices.iter().find(|d| d.device_name == name).cloned();
        }
        if st.device.is_none() {
            st.device = st.devices.first().cloned();
        }
    }

    rebuild_device_combo(ctx);
    refresh_option_dropdowns(ctx);
    refresh_profiles(ctx);

    // Выбранное устройство: запросить возможности.
    if let Some(dev) = ctx.state.borrow().device.clone() {
        let id = ctx
            .worker
            .request(Request::Capabilities { device: dev.device_name });
        ctx.pending.borrow_mut().insert(id, Pending::Capabilities);
        set_status(ctx, &dev.name, false);
    }

    // Автопоиск устройств при старте.
    refresh_devices(ctx);
}
