//! What the Rust Obsidian plugins in this folder share: calling JavaScript by name (`js`: `call!`, `obj!`, closures,
//! timers, promises) and the settings tab (`settings`). Each plugin is its own crate, compiled to WebAssembly and loaded
//! by a short main.js; `pack` (../pack) builds any of them.

pub mod gpu;
pub mod js;
pub mod settings;

// For the macros, so a plugin needs no direct dependency on these to use them.
pub use js_sys;
pub use wasm_bindgen;
