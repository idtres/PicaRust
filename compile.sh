#!/usr/bin/env bash
# Comprueba que Rust esté instalado y compila PicaRust en modo release.
#   ./compile.sh            compila
#   ./compile.sh --install  compila e instala en ~/.local/bin y lo registra
#                           como visor de imágenes (asociación de archivos)
set -euo pipefail
cd "$(dirname "$0")"

# Rust instalado con rustup suele estar en ~/.cargo/bin aunque no esté en el PATH.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

if ! command -v cargo >/dev/null 2>&1; then
    echo "✗ Rust (cargo) no está instalado."
    read -r -p "¿Instalarlo ahora con rustup? [s/N] " r
    if [[ "$r" =~ ^[sSyY]$ ]]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        . "$HOME/.cargo/env"
    else
        echo "Instálalo con:  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi
fi
echo "✓ $(rustc --version)"
echo "✓ $(cargo --version)"

echo "→ Compilando (release)…"
cargo build --release

BIN="target/release/picarust"
echo "✓ Listo: $BIN ($(du -h "$BIN" | cut -f1))"

if [[ "${1:-}" == "--install" ]]; then
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
    install -m 755 "$BIN" "$HOME/.local/bin/picarust"
    sed "s|^Exec=.*|Exec=$HOME/.local/bin/picarust %f|" picarust.desktop \
        > "$HOME/.local/share/applications/picarust.desktop"
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
    for m in image/jpeg image/png; do
        xdg-mime default picarust.desktop "$m" 2>/dev/null || true
    done
    echo "✓ Instalado en ~/.local/bin/picarust y asociado a JPG/JPEG/PNG"
fi
