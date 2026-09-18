#!/usr/bin/env bash
# Сборка бинарного RPM для РЕД ОС / RPM-дистрибутивов БЕЗ Docker:
# нужны только rpmbuild, tar и уже собранные бинарники.
#
# Использование:
#   scripts/build-rpm.sh <каталог-с-бинарниками> [номер-релиза]
#
# Примеры:
#   scripts/build-rpm.sh target/release 9
#   scripts/build-rpm.sh target/x86_64-unknown-linux-gnu/release 9
#
# Готовый RPM копируется в корень репозитория:
#   scanner-app-<версия>-<номер>.x86_64.rpm
# Номер релиза совпадает с номером выпуска (тег v0.2.0-N).
set -euo pipefail

BIN_DIR="${1:-}"
REL="${2:-1}"

if [[ -z "$BIN_DIR" ]]; then
    echo "Использование: $0 <каталог-с-бинарниками> [номер-релиза]" >&2
    exit 2
fi

if ! command -v rpmbuild >/dev/null 2>&1; then
    echo "rpmbuild не найден." >&2
    echo "  РЕД ОС / Fedora:  sudo dnf install rpm-build" >&2
    echo "  Debian / Ubuntu:  sudo apt install rpm" >&2
    exit 3
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VER="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
echo "==> Версия ${VER}, Release ${REL}, бинарники: ${BIN_DIR}"

for b in scanner-gui scanner-worker scanner-cli; do
    if [[ ! -x "${BIN_DIR}/${b}" ]]; then
        echo "Нет исполняемого файла ${BIN_DIR}/${b} — сначала соберите проект." >&2
        exit 4
    fi
done

TOPDIR="${ROOT}/rpmbuild"
SOURCES="${TOPDIR}/SOURCES"
rm -rf "$TOPDIR"
mkdir -p "$SOURCES" "${TOPDIR}/BUILD" "${TOPDIR}/BUILDROOT" \
         "${TOPDIR}/RPMS" "${TOPDIR}/SRPMS"

# Source0: тар с ГОТОВЫМИ бинарниками + лицензия/документация для %license/%doc.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
install -m 0755 "${BIN_DIR}/scanner-gui"    "${STAGE}/scanner-gui"
install -m 0755 "${BIN_DIR}/scanner-worker" "${STAGE}/scanner-worker"
install -m 0755 "${BIN_DIR}/scanner-cli"    "${STAGE}/scanner-cli"
install -m 0644 LICENSE README.md "$STAGE/"

tar -czf "${SOURCES}/scanner-app-${VER}-bin.tar.gz" \
    --transform "s,^,scanner-app-${VER}/," \
    -C "$STAGE" scanner-gui scanner-worker scanner-cli LICENSE README.md

cp packaging/scanner-app.desktop packaging/scanner-app.svg "$SOURCES/"

echo "==> rpmbuild -bb packaging/scanner-app.spec --define \"rel_num ${REL}\""
rpmbuild -bb packaging/scanner-app.spec \
    --define "_topdir ${TOPDIR}" \
    --define "rel_num ${REL}"

RPM_PATH="$(find "${TOPDIR}/RPMS" -name '*.rpm' -print -quit)"
if [[ -z "$RPM_PATH" ]]; then
    echo "rpmbuild не создал RPM" >&2
    exit 5
fi
RPM="${ROOT}/$(basename "$RPM_PATH")"
cp -f "$RPM_PATH" "$RPM"

echo
echo "==> Готово: ${RPM}"
echo "==> Метаданные:"; rpm -qpi "$RPM" | sed -n '1,14p'
echo "==> Requires:";    rpm -qpR "$RPM"
echo "==> Состав:";      rpm -qpl "$RPM"

# Диагностика для офлайн-установки на РЕД ОС: самая «свежая» версия символа
# glibc/glib, которую требуют бинарники (должна быть не выше glibc целевой ОС).
if command -v objdump >/dev/null 2>&1; then
    echo "==> Максимальные версии символов glibc/glib:"
    for b in scanner-gui scanner-worker scanner-cli; do
        NEED="$(objdump -T "${BIN_DIR}/${b}" 2>/dev/null \
                | grep -oE 'GLIBC_[0-9]+\.[0-9]+|GLIB_[0-9]+\.[0-9]+' \
                | sort -Vu | tail -n1 || true)"
        printf '    %-16s %s\n' "$b" "${NEED:-нет}"
    done
fi