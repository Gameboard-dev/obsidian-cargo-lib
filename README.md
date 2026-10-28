# obsidian-cargo-lib

A Rust workspace for Obsidian plugins written in Rust and compiled to WebAssembly, with no JavaScript tooling: each plugin
ships an 18-line `main.js` that loads its `.wasm` and hands it Obsidian, and the `.wasm` carries its own wasm-bindgen
bindings in a custom section.

- `shared/`: the crate `obsidian-kit`, compiled into each plugin: calling JavaScript by name through `Reflect` (`call!`,
  `obj!`, closures, futures, timers) and a settings tab declared as data (`shared/README.md`).
- `pack/`: the build: compiles a plugin to WebAssembly (with SIMD), runs wasm-bindgen, stores the bindings in the
  `.wasm`, optionally builds a plugin's native server, and copies the plugin into a vault.

The plugins are repositories of their own, cloned into this folder; every `*/core` crate present is a member of the
workspace:

| Plugin | Repository |
|---|---|
| Augmented View | <https://github.com/Gameboard-dev/obsidian-augmented-view> |
| Dictionary and Synonyms | <https://github.com/Gameboard-dev/dictionary-and-synonyms> |
| LaTeX | <https://github.com/Gameboard-dev/latex> |

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129     # must match the wasm-bindgen crate
git clone https://github.com/Gameboard-dev/obsidian-cargo-lib && cd obsidian-cargo-lib
git clone https://github.com/Gameboard-dev/latex latex
cargo run -p pack --release -- latex --deploy <vault>/.obsidian/plugins/latex
```

Machine-specific build settings (a CUDA compiler path, `target-cpu=native`) go in `.cargo/config.toml` here, which is
not tracked.

## Licence

MIT (`LICENSE`).
