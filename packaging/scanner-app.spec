# RPM spec для РЕД ОС / RPM-based дистрибутивов.
# Пакетирует ЗАРАНЕЕ СОБРАННЫЕ бинарники (release), поэтому на целевой
# машине не нужны rust/cargo/GTK-devel — RPM устанавливается офлайн:
#   sudo rpm -Uvh scanner-app-0.2.0-9.x86_64.rpm
# SANE-стек (sane-backends/sane-airscan/ipp-usb) — жёсткий Requires:
# без него приложение не может сканировать. Жёстких ELF-зависимостей
# (GTK и т.п.) нет, чтобы установка проходила офлайн.
#
# Сборка БЕЗ Docker (только rpmbuild):
#   scripts/build-rpm.sh target/release 9
# вручную:
#   rpmbuild -bb packaging/scanner-app.spec \
#     --define "rel_num 9" --define "_topdir $PWD/rpmbuild"

%global _enable_debug_package 0
%global debug_package %{nil}

# Release-номер RPM = номер выпуска (тег v0.2.0-N) и приходит от сборочного
# скрипта/CI как --define "rel_num N". Если не задан — 1.
%{!?rel_num: %global rel_num 1}

Name:           scanner-app
Version:        0.2.0
Release:        %{rel_num}%{?dist}
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
* Fri Sep 18 2026 Scanner App Team <dev@example.local> - 0.2.0-9
- Воркер: главный цикл больше не блокируется операциями SANE (scanimage -L
  занимал до 90 с и «подвешивал» кнопки «Сканировать»/«Прервать»);
  Cancel/Ping/Shutdown обрабатываются мгновенно, тяжёлые операции идут в
  фоновых потоках и сериализуются
- Сканирование: два задания больше не борются за устройство (раньше
  оставались зависшие процессы scanimage, и следующие сканы не стартовали)
- Сканирование: сторожевой таймер — если сканер 5 минут не отдаёт данные,
  задание завершается понятной ошибкой вместо бесконечного ожидания
- GUI: авто-перезапуск упавшего scanner-worker, защита от повторного
  запуска скана/предпросмотра, сброс «зависших» запросов при падении воркера
- GUI: явные цвета текста в списках и кнопках (на части тем РЕД ОС подписи
  были невидимы), кнопка «Прервать» выделена красным
- RPM: номер Release следует за тегом (--define rel_num); сборка RPM без
  Docker (scripts/build-rpm.sh), метаданные проверяются в CI

* Tue Sep 16 2025 Scanner App Team <dev@example.local> - 0.2.0-6
- GUI: выбор рендерера вынесен в проверяемую функцию (тесты: cairo по
  умолчанию, SCANNER_GL=1 — аппаратный, свой GSK_RENDERER — приоритетнее)
- GUI: выбранный рендерер пишется в журнал приложения (диагностика
  «пустого окна» при запуске с ярлыка)
- CI: тесты запускаются по всему workspace
* Mon Sep 15 2025 Scanner App Team <dev@example.local> - 0.2.0-5
- GUI: аппаратные рендереры GTK4 по умолчанию отключены (cairo) — на части
  систем РЕД ОС вызывали зависание сессии и пустые виджеты; SCANNER_GL=1
  возвращает аппаратный рендеринг
- GUI: предпросмотр и диалог обрезки рендерятся в фоновом потоке (декодирование
  скана 600 DPI в главном потоке подвешивало интерфейс)
- Ярлык упрощён (рендерер задаёт само приложение)

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
