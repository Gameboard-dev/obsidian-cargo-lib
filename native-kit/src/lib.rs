//! What the native servers of the plugins in this folder share (Augmented View's model server, Scholar's service): the
//! hardware they run on (`hw`). A server is its own program and workspace; this crate is a path dependency of each, with
//! the same features (`cuda`, `metal`, and `candle` for one that places candle models).

pub mod hw;
