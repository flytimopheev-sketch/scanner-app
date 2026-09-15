#!/usr/bin/env bash
# Сборка «Сканера документов» (РЕД ОС / любой Linux с GTK4).
# Использование:
#   ./packaging/build.sh              # debug-сборка
#   ./packaging/build.sh --release    # release-сборка
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="debug"
if [[ "${1:-}" == "--release" ]]; then
    PROFILE="release"
fi

# Пакеты для сборки (РЕД ОС / Fedora / ALT):
#   sudo dnf install rust cargo clang gtk4-devel libadwaita-devel \
#                    sane-backends sane-airscan ipp-usb
# Для FFI-бэкенда libsane дополнительно: sane-backends-devel
# и собрать с фичей: cargo build --features sane-ffi

echo "==> cargo build --$PROFILE"
cargo build "--$PROFILE"

BIN_DIR="target/$PROFILE"
echo
echo "==> Готово:"
echo "    $BIN_DIR/scanner-gui     — GUI"
echo "    $BIN_DIR/scanner-worker  — воркер SANE (кладите рядом с GUI)"
echo
echo "==> Проверка окружения SANE на машине со сканером:"
echo "    scanimage -L          # обнаружение устройств"
echo "    scanimage -T          # тест устройства"
echo "    airscan-discover      # сетевые eSCL/WSD-устройства"
echo
echo "==> Запуск:"
echo "    $BIN_DIR/scanner-gui"
echo
echo "    Без сканера можно смотреть интерфейс на мок-устройствах:"
echo "    SCANNER_BACKEND=mock $BIN_DIR/scanner-gui"
