#!/usr/bin/env bash
# Сборка релизных бинарников Гефеста для npm-дистрибуции.
#
# Использование:
#   ./build-release.sh                 # текущая платформа → npm/bin/
#   ./build-release.sh all             # все платформы, доступные на этой машине
#
# Результат: dist/<triple>.tar.gz с бинарником внутри (формат для postinstall.mjs)
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
mkdir -p dist

build_native() {
  echo "=== Сборка нативного бинаря (release) ==="
  cargo build --release
  local triple
  triple=$(rustc -vV | grep host | awk '{print $2}')
  local bin="target/release/hephaestus-rs"
  [ -f "$bin" ] || bin="target/release/hephaestus"
  local tmp="dist/hephaestus-$triple"
  rm -rf "$tmp" && mkdir -p "$tmp"
  cp "$bin" "$tmp/hephaestus"
  tar -czf "dist/${triple}.tar.gz" -C "$tmp" hephaestus
  echo "Готово: dist/${triple}.tar.gz"
}

case "${1:-native}" in
  native)
    build_native
    ;;
  windows)
    echo "=== Кросс-сборка Windows (x86_64-pc-windows-gnu) ==="
    rustup target add x86_64-pc-windows-gnu
    cargo build --release --target x86_64-pc-windows-gnu
    tmp="dist/hephaestus-x86_64-pc-windows-gnu"
    rm -rf "$tmp" && mkdir -p "$tmp"
    cp "target/x86_64-pc-windows-gnu/release/hephaestus-rs.exe" "$tmp/hephaestus.exe"
    tar -czf "dist/x86_64-pc-windows-gnu.tar.gz" -C "$tmp" hephaestus.exe
    echo "Готово: dist/x86_64-pc-windows-gnu.tar.gz"
    ;;
  all)
    build_native
    ;;
  *)
    echo "использование: $0 [native|windows|all]"
    exit 1
    ;;
esac

echo
echo "Дальше: опубликовать npm/hephaestus-agent (обёртка) и залить dist/*.tar.gz на release-сервер."
