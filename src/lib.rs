//! SpesaApp: shopping list web app (Leptos full-stack).
//!
//! - [`model`]: types and logic shared by server and browser
//! - [`app`]: user interface (server-side rendered, hydrated in the browser)
//! - [`api`]: server functions called by the UI
//! - [`db`]: SQLite access (server only)
//! - [`alexa`]: import from the Alexa shopping list (server only)

pub mod api;
pub mod app;
pub mod icons;
pub mod model;

#[cfg(feature = "ssr")]
pub mod alexa;
#[cfg(feature = "ssr")]
pub mod db;

/// Entry point of the WASM bundle: makes the server-rendered page interactive.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}
