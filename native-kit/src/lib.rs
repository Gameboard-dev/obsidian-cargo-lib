//! What the native servers of the plugins in this folder share (Augmented View's model server, Scholar's service): the
//! hardware they run on (`hw`). A server is its own program and workspace; this crate is a path dependency of each, with
//! the same features (`cuda`, `metal`, and `candle` for one that places candle models). With `candle`: Qwen3 weights as
//! GGUF (`gguf`); with `models`: model files from the Hugging Face cache or downloaded (`models`).

pub mod hw;
#[cfg(feature = "candle")]
pub mod gguf;
#[cfg(feature = "models")]
pub mod models;
