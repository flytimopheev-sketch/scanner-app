# RPM spec для РЕД ОС / RPM-based дистрибутивов.
# Пакетирует ЗАРАНЕЕ СОБРАННЫЕ бинарники (release), поэтому на целевой
# машине не нужны rust/cargo/GTK-devel — RPM устанавливается офлайн:
#   sudo rpm -Uvh scanner-app-0.2.0-1.*.rpm
# SANE-стек (sane-backends/sane-airscan/ipp-usb) — жёсткий Requires:
# без него приложение не может сканировать. Жёстких ELF-зависимостей
# (GTK и т.п.) нет, чтобы установка проходила офлайн.
# Сборка: rpmbuild -bb packaging/scanner-app-bin.spec

%global _enable_debug_package 0
%global debug_package %{nil}

Name:           scanner-app
Version:        0.2.0
Release:        4%{?dist}
Summary:        Document scanner frontend for SANE (NAPS2-like)
License:        GPL-3.0-or-later
URL:            https://example.local/scanner-app
BuildArch:      x86_64

# Source0 — tar с собранными бинарниками scanner-gui/worker/cli в корне
Source0:        %{name}-%{version}-bin.tar.gz
Source1:        scanner-app.desktop
Source2:        scanner-app.svg

# Не генерировать жёсткие ELF-зависимости по библиотекам (gtk4/libadwaita
# и т.п.) — иначе офлайн-установка на РЕД ОС падает. GTK4/libadwaita есть
# в любой РЕД ОС с графическим окружением. SANE-стек — Requires (см. выше).
%global __requires_exclude_from ^%{_bindir}/.*$

# Совместимый payload (gzip): zstd не читается rpm < 4.14 (старые РЕД ОС).
%global _binary_payload w9.gzdio

Requires:       sane-backends
Requires:       sane-airscan
Recommends:     ipp-usb
Recommends:     tesseract
Recommends:     tesseract-rus
Recommends:     xdg-email

%description
Универсальный фронтенд для SANE: USB-сканеры, сетевые устройства
(eSCL/AirScan, WSD), МФУ с автоподатчиком (ADF), аппаратным и ручным
дуплексом. Профили сканирования, обработка страниц (поворот, интерактивная
обрезка, авто-выправление, автоконтраст, пропуск пустых страниц), OCR
через локальный Tesseract (поисковый PDF), экспорт PDF/PNG/JPEG/TIFF,
шаблоны имён, отправка в папку/почту, наблюдаемая папка, консольный
режим scanner-cli. GUI отделён от scanner-worker, что защищает интерфейс
от зависаний и падений драйверов.

%prep
%setup -q

%install
install -Dm 0755 scanner-gui    %{buildroot}%{_bindir}/scanner-gui
install -Dm 0755 scanner-worker %{buildroot}%{_bindir}/scanner-worker
install -Dm 0755 scanner-cli    %{buildroot}%{_bindir}/scanner-cli
install -Dm 0644 %{SOURCE1} %{buildroot}%{_datadir}/applications/%{name}.desktop
install -Dm 0644 %{SOURCE2} %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%files
%license LICENSE
%doc README.md
%{_bindir}/scanner-gui
%{_bindir}/scanner-worker
%{_bindir}/scanner-cli
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%changelog
* Mon Sep 15 2025 Scanner App Team <dev@example.local> - 0.2.0-4
- Ярлык: GSK_RENDERER=cairo — GL-рендерер GTK4 на РЕД ОС рисует пустые виджеты;
  программный cairo отрисовывает интерфейс гарантированно

* Mon Sep 15 2025 Scanner App Team <dev@example.local> - 0.2.0-3
- GUI: привязаны к действиям кнопки «Сканировать», «Прервать», «Предпросмотр»,
  «Загрузить изображения», «Сохранить PDF», «Сохранить профиль» и др.
- Ярлык: GSK_RENDERER=gl (окно не появлялось при запуске с рабочего стола),
  добавлен StartupWMClass

* Mon Sep 15 2025 Scanner App Team <dev@example.local> - 0.2.0-2
- GUI: кнопки «+» и «Обновить список» не были привязаны к действиям — исправлено
- sane-backends/sane-airscan/ipp-usb переведены в Requires (rpm не ставит Recommends)
- Понятная ошибка, если утилита scanimage не установлена

* Mon Sep 15 2025 Scanner App Team <dev@example.local> - 0.2.0-1
- Бинарный RPM: сборка без rust/cargo/GTK-devel на целевой машине
- Мягкие зависимости (Recommends) — установка работает офлайн
- CLI: исправлен игнор имени файла из --out

* Fri Sep 05 2025 Scanner App Team <dev@example.local> - 0.2.0
- OCR через локальный Tesseract (поисковый PDF, merge постраничных PDF)
- Ручной дуплекс «лицевая/оборотная» с диалогом переворота стопа
- Автопропуск пустых страниц с настраиваемым порогом
- Предпросмотр планшета в низком DPI (не попадает в документ)
- Drag&drop переупорядочивание страниц, анимация сканирования
- Статус устройств (доступен/недоступен) в списке и шапке
- Интерактивная обрезка ручками по углам; заголовки у всех диалогов
- Диалог экспорта: шаблоны имён, OCR, отправка в папку/почту, подпись ГОСТ
- Наблюдаемая папка (запись МФУ), внешние переводы (JSON), scanner-cli
- Новый бинарник scanner-cli; сборка требует libadwaita >= 1.4

* Wed Sep 03 2025 Scanner App Team <dev@example.local> - 0.1.0
- Initial package: scanner-core, scanner-worker, scanner-gui (GTK4/libadwaita)
