#!/usr/bin/env bash
# Скриншот-сценарий интерфейса: сборка + запуск под Xvfb в режиме
# SCANNER_SHOTS. Используется в CI и для документации.
#
# Использование: bash scripts/screenshots.sh <выходной-каталог>
set -euo pipefail

OUT="${1:-screenshots}"
mkdir -p "$OUT"

# Инструменты снимков: xwd + xwdtopnm/pnmtopng (netpbm).
command -v Xvfb >/dev/null || { echo "Xvfb не найден"; exit 1; }
command -v xwd >/dev/null || { echo "xwd не найден"; exit 1; }
command -v xwdtopnm >/dev/null || { echo "xwdtopnm (netpbm) не найден"; exit 1; }

cargo build -p scanner-gui -p scanner-worker

export DISPLAY="${DISPLAY:-:99}"
Xvfb "$DISPLAY" -screen 0 1280x860x24 -nolisten tcp &
XVFB_PID=$!
trap 'kill $XVFB_PID 2>/dev/null || true' EXIT
sleep 2

# D-Bus-сессия — для libadwaita/gsettings (тема, тосты).
if command -v dbus-run-session >/dev/null; then
  exec dbus-run-session -- bash -c "
    SCANNER_SHOTS='$OUT' SCANNER_BACKEND=mock \
    GDK_PIXBUF_MODULE_FILE='${GDK_PIXBUF_MODULE_FILE:-}' \
    target/debug/scanner-gui > /tmp/scanner-shots.log 2>&1
  "
else
  SCANNER_SHOTS="$OUT" SCANNER_BACKEND=mock \
    target/debug/scanner-gui > /tmp/scanner-shots.log 2>&1
fi

echo "Снимки: $OUT"
