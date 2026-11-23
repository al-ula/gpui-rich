//! WebAssembly browser entrypoint for the `gpui-rich` showcase application.

#[path = "../../docs_viewer.rs"]
#[allow(dead_code)]
mod docs_viewer;

#[cfg(target_family = "wasm")]
mod web_entry {
    use crate::docs_viewer::DocsViewerApp;
    use gpui_kit::web::{CanvasFontFallback, WebBackendPreference, WebPlatform};
    use gpui_kit::{App, AppContext, Application, WindowOptions};
    use gpui_rich::gpui::{clear_font_resolution_cache, register_default_keybindings};
    use std::borrow::Cow;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    fn hide_loading_overlay() {
        if let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Some(loading_element) = document.get_element_by_id("loading")
        {
            loading_element.remove();
        }
    }

    fn show_error_overlay(message: &str) {
        if let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Some(status_element) = document.get_element_by_id("loading-status")
        {
            status_element.set_text_content(Some(message));
        }
    }

    /// Starts the `gpui-rich` web viewer with font binaries fetched by the host page.
    ///
    /// Pass an array of `Uint8Array` font files including `IBM Plex Sans` (required for
    /// GPUI's `.SystemUIFont` fallback on WebAssembly).
    #[wasm_bindgen]
    pub fn start(fonts: js_sys::Array) -> Result<(), JsValue> {
        gpui_kit::platform::web_init();

        let mut font_blobs: Vec<Cow<'static, [u8]>> = Vec::with_capacity(fonts.length() as usize);
        for index in 0..fonts.length() {
            let entry = fonts.get(index);
            let bytes = entry.dyn_into::<js_sys::Uint8Array>().map_err(|_| {
                JsValue::from_str(&format!(
                    "expected Uint8Array for font entry at index {index}"
                ))
            })?;
            font_blobs.push(Cow::Owned(bytes.to_vec()));
        }

        if font_blobs.is_empty() {
            return Err(JsValue::from_str(
                "at least one font (IBM Plex Sans) must be loaded before starting GPUI on web",
            ));
        }

        // Single-threaded mode works on any static HTTP server without COOP/COEP headers.
        // EmojiAndCjk lets the browser Canvas 2D API supply emoji and CJK fallback glyphs.
        let platform = Rc::new(WebPlatform::new_with_backend_and_font_fallback(
            false,
            WebBackendPreference::Auto,
            CanvasFontFallback::EmojiAndCjk,
        ));
        let application = Application::with_platform(platform);

        application.run(move |cx: &mut App| {
            gpui_kit::init(cx);
            register_default_keybindings(cx);

            if let Err(error) = cx.text_system().add_fonts(font_blobs) {
                log::error!("docs_viewer_web: failed to register web fonts: {error}");
            }
            clear_font_resolution_cache();

            match gpui_kit::open_window(WindowOptions::default(), cx, |_window, cx| {
                cx.new(DocsViewerApp::new)
            }) {
                Ok(_) => {
                    hide_loading_overlay();
                    cx.activate(true);
                }
                Err(error) => {
                    log::error!("docs_viewer_web: failed to open window: {error}");
                    show_error_overlay(&format!("Failed to open GPUI window: {error}"));
                }
            }
        });

        Ok(())
    }
}
