//! Sparrow Engine: Rust browser engine embedded in the Sparrow shell.

// --- core pipeline: parse -> style -> layout -> paint
pub mod dro_charset;
pub mod dro_css;
pub mod dro_css_ext;
pub mod dro_dom;
pub mod dro_dom_js;
pub mod dro_html;
pub mod dro_layout;
pub mod dro_paint;

// --- scripting
pub mod dro_js;
pub mod js_engine;

// --- network, fonts, media, blocking
pub mod dro_blocklist;
pub mod dro_font;
pub mod dro_media;
pub mod dro_media_ui;
pub mod dro_net;

// --- engine facade, C ABI, safety, browser features
pub mod browser;
pub mod ffi;
pub mod safety;
pub mod sparrow_engine;

pub use sparrow_engine::SparrowEngine;

pub fn create_engine() -> SparrowEngine {
    SparrowEngine::new()
}
