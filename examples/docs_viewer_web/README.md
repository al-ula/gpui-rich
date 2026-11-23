# `gpui-rich` Web Viewer Example

Browser (WebAssembly + WebGPU/WebGL2) build of the `docs_viewer` example sharing `DocsViewerApp` directly with `examples/docs_viewer.rs`.

## Prerequisites

- Rust `nightly` toolchain with the `wasm32-unknown-unknown` target (configured automatically by `rust-toolchain.toml` in this directory)
- `wasm-bindgen-cli` matching the crate's `wasm-bindgen` version:

```bash
rustup toolchain install nightly --target wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli
```

## Build & Run

```bash
cd examples/docs_viewer_web
./build.sh --serve
```

Open `http://127.0.0.1:8080` in a WebGPU- or WebGL2-capable browser.

### Options

- `./build.sh --release --serve`: Optimized release WASM build.
- `./build.sh --gpui-fast`: Build with the `gpui-fast` backend feature.
- `./build.sh --fetch-fonts --serve`: Download fonts into `www/fonts/` once so the page loads fonts locally without CDN requests.

## Fonts

GPUI's browser platform starts with an empty font database and resolves `.SystemUIFont` to `IBM Plex Sans`. `www/index.html` fetches `IBM Plex Sans`, `IBM Plex Serif`, `IBM Plex Mono`, `Noto Sans Hebrew`, `Noto Sans Arabic`, and `Noto Sans Symbols 2` at runtime (checking `./fonts/` first and falling back to jsDelivr) before initializing the GPUI window, keeping the `.wasm` artifact small while providing full bold, italic, serif, monospace, UI symbol, and BiDi glyph coverage. Emoji and CJK characters fall back to the browser's Canvas 2D renderer.
