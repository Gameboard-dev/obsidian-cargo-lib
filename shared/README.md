# obsidian-kit

What the Rust Obsidian plugins in `obsidian-plugins/` share (Augmented View, Dictionary and Synonyms), compiled into each plugin's .wasm:

- `src/js.rs`: calling JavaScript by name through `Reflect`, with no web-sys: `call!(obj, method, args…)`, `obj!{ key: value, … }`, `get`/`set`, closures (`f0`, `f1`, …), `spawn` (a future on the JavaScript event loop), `timeout`, and `sleep` (a future, for retries and pacing).
- `src/settings.rs`: a settings tab declared as data. A plugin lists `Row`s (key, name, description, control: `Head`, `Note`, `Toggle`, `Slider`, `Text`, `Number`, `Choice(options)`, or `Custom(fn)` for a row it fills itself), and `render(tab, obsidian, rows, get_value, set_value)` builds it with Obsidian's `Setting`.

Each plugin depends on it with `obsidian-kit = { path = "../../shared" }` and re-exports `obsidian_kit::js` as its own `js` module. The crate re-exports `wasm_bindgen` and `js_sys`, so its macros work in any plugin crate.
