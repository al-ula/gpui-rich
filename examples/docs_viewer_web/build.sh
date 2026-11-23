#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PROFILE="debug"
CARGO_FLAGS=()
FETCH_FONTS=0
SERVE=0
PORT="8080"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      PROFILE="release"
      CARGO_FLAGS+=("--release")
      ;;
    --gpui-fast)
      CARGO_FLAGS+=("--features" "gpui-fast")
      ;;
    --fetch-fonts)
      FETCH_FONTS=1
      ;;
    --serve)
      SERVE=1
      ;;
    --port)
      shift
      PORT="${1:-8080}"
      ;;
    -h|--help)
      cat <<'EOF'
Usage: ./build.sh [options]

Options:
  --release      Build the WebAssembly binary in release mode
  --gpui-fast    Enable the gpui-fast backend feature
  --fetch-fonts  Download fonts into www/fonts/ for offline local serving
  --serve        Start a local HTTP server after building
  --port PORT    Port for --serve (default: 8080)
EOF
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
  shift
done

if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "error: wasm-bindgen-cli is required. Install with: cargo install --locked wasm-bindgen-cli" >&2
  exit 1
fi

if [[ "$FETCH_FONTS" -eq 1 ]]; then
  mkdir -p www/fonts
  declare -A FONT_URLS=(
    ["IBMPlexSans-Regular.ttf"]="https://cdn.jsdelivr.net/gh/IBM/plex@master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-Regular.ttf"
    ["IBMPlexSans-SemiBold.ttf"]="https://cdn.jsdelivr.net/gh/IBM/plex@master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-SemiBold.ttf"
    ["IBMPlexSans-Bold.ttf"]="https://cdn.jsdelivr.net/gh/IBM/plex@master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-Bold.ttf"
    ["IBMPlexSans-Italic.ttf"]="https://cdn.jsdelivr.net/gh/IBM/plex@master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-Italic.ttf"
    ["IBMPlexSans-BoldItalic.ttf"]="https://cdn.jsdelivr.net/gh/IBM/plex@master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-BoldItalic.ttf"
    ["IBMPlexSerif-Regular.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexserif/IBMPlexSerif-Regular.ttf"
    ["IBMPlexSerif-Bold.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexserif/IBMPlexSerif-Bold.ttf"
    ["IBMPlexSerif-Italic.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexserif/IBMPlexSerif-Italic.ttf"
    ["IBMPlexSerif-BoldItalic.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexserif/IBMPlexSerif-BoldItalic.ttf"
    ["IBMPlexMono-Regular.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexmono/IBMPlexMono-Regular.ttf"
    ["IBMPlexMono-Bold.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexmono/IBMPlexMono-Bold.ttf"
    ["IBMPlexMono-Italic.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/ibmplexmono/IBMPlexMono-Italic.ttf"
    ["NotoSansHebrew.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/notosanshebrew/NotoSansHebrew%5Bwdth%2Cwght%5D.ttf"
    ["NotoSansArabic.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/notosansarabic/NotoSansArabic%5Bwdth%2Cwght%5D.ttf"
    ["NotoSansSymbols2-Regular.ttf"]="https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/notosanssymbols2/NotoSansSymbols2-Regular.ttf"
  )
  for filename in "${!FONT_URLS[@]}"; do
    if [[ ! -f "www/fonts/$filename" ]]; then
      echo "Downloading www/fonts/$filename..."
      curl -fsSL "${FONT_URLS[$filename]}" -o "www/fonts/$filename"
    fi
  done
fi

echo "Building docs-viewer-web ($PROFILE) for wasm32-unknown-unknown..."
# GPUI's render tree in debug WASM builds can exceed wasm-ld's 1 MiB default stack.
cargo rustc --target wasm32-unknown-unknown -j 4 "${CARGO_FLAGS[@]}" -- \
  -C link-arg=-zstack-size=8388608

WASM_PATH="target/wasm32-unknown-unknown/$PROFILE/docs_viewer_web.wasm"
mkdir -p www/pkg

echo "Generating JavaScript bindings in www/pkg/..."
wasm-bindgen "$WASM_PATH" \
  --out-dir www/pkg \
  --target web \
  --no-typescript

echo "Built web bundle in $SCRIPT_DIR/www"

if [[ "$SERVE" -eq 1 ]]; then
  echo "Serving http://127.0.0.1:$PORT"
  python3 -m http.server "$PORT" --directory www
fi
