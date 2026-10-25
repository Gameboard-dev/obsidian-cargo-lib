//! `cargo run -p pack --release -- <plugin> [--fast] [--server] [--cpu-server] [--deploy DIR]`, from this folder's
//! parent: builds a plugin's crate (`<plugin>/core`) to WebAssembly, generates its JavaScript bindings with
//! wasm-bindgen, and stores them inside the .wasm as a custom section named "glue", so main.js stays the only JavaScript
//! file. Writes `<plugin>/<plugin_with_underscores>.wasm` (augmented_view.wasm, dictionary_and_synonyms.wasm).
//!
//! - `--fast` builds with the `fast` profile (thin LTO, incremental; ../Cargo.toml) instead of `release`.
//! - `--server` also builds the plugin's server (`<plugin>/server`, Augmented View's; `<plugin>/service`, Scholar's) for this
//!   computer: CUDA, or Metal on macOS.
//! - `--cpu-server` also builds its CPU-only version, into server/target/cpu (Augmented View's fallback, `cpuServer`).
//! - `--deploy DIR` copies the plugin into DIR, a vault's `.obsidian/plugins/<plugin>` (or the folder in the environment
//!   variable `<PLUGIN>_DEPLOY`, e.g. AUGMENTED_VIEW_DEPLOY). Servers are not copied: point the plugin's settings at them.
//! - `--release DIR` writes the release's assets into DIR: main.js, manifest.json and styles.css, the only files
//!   Obsidian installs from a release. A main.js with the line `const WASM = null;` gets the module there, base64-encoded
//!   (so does a deployed one), and the .wasm is then not needed beside it.
//! - Nothing is regenerated when the compiled module is unchanged (its hash is kept in target/pack/<plugin>.stamp).
//!
//! WebAssembly SIMD (simd128) is on for every plugin: every runtime they meet has it (Obsidian's Electron, Chrome/Edge
//! 91+, Firefox 89+, Safari 16.4+; an older Safari, e.g. Ignis on an iPhone before iOS 16.4, would fail to load the
//! module). Measured 2026-10-24 on a 1.6 MB, 5,000-paragraph note: xxh3 hashing 0.133 -> 0.070 ms; small, but free.
//!
//! References: wasm-bindgen's CLI and its `no-modules` target
//! (<https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html#without-a-bundler>); custom sections in the
//! WebAssembly binary format (<https://webassembly.github.io/spec/core/binary/modules.html#custom-section>), read back
//! by `WebAssembly.Module.customSections`
//! (<https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/Module/customSections_static>);
//! `cargo --config` (<https://doc.rust-lang.org/cargo/reference/config.html#command-line-overrides>).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::{fs, path::Path, process::Command};

const SIMD: &str = "target.wasm32-unknown-unknown.rustflags=[\"-C\", \"target-feature=+simd128\"]";

fn run(cmd: &mut Command) {
    let ok = cmd.status().unwrap_or_else(|e| panic!("{cmd:?}: {e}")).success();
    assert!(ok, "{cmd:?} failed");
}

/// main.js with the module written into its `const WASM = null;` line, base64-encoded (RFC 4648,
/// <https://www.rfc-editor.org/rfc/rfc4648#section-4>); as it is when it has no such line.
fn bundle(main_js: &Path, wasm: &Path) -> String {
    let js = fs::read_to_string(main_js).unwrap();
    if !js.contains("const WASM = null;") {
        return js;
    }
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = fs::read(wasm).unwrap();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    js.replace("const WASM = null;", &format!("const WASM = \"{out}\";"))
}

/// Unsigned LEB128, as WebAssembly encodes sizes (<https://webassembly.github.io/spec/core/binary/values.html#integers>).
fn leb(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        out.push(if n == 0 { b } else { b | 0x80 });
        if n == 0 {
            return;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().filter(|a| !a.starts_with("--")).cloned().expect("which plugin? cargo run -p pack --release -- <plugin folder, e.g. augmented-view>");
    let flag = |f: &str| args.iter().any(|a| a == f);
    let profile = if flag("--fast") { "fast" } else { "release" };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let plugin = root.join(&name);
    let (krate, stem) = (format!("{name}-core"), name.replace('-', "_"));
    let out = root.join("target/pack").join(&name);
    let target = plugin.join(format!("{stem}.wasm"));
    run(Command::new("cargo").current_dir(root).args(["build", "--config", SIMD, "--profile", profile, "--target", "wasm32-unknown-unknown", "-p", &krate]));

    let compiled = root.join(format!("target/wasm32-unknown-unknown/{profile}/{stem}_core.wasm"));
    let mut h = DefaultHasher::new();
    fs::read(&compiled).unwrap().hash(&mut h);
    let stamp = format!("{profile} {:016x}", h.finish());
    let stamp_file = root.join(format!("target/pack/{name}.stamp"));
    if fs::read_to_string(&stamp_file).ok().as_deref() == Some(stamp.as_str()) && target.exists() {
        println!("{stem}.wasm: unchanged");
    } else {
        run(Command::new("wasm-bindgen").args(["--target", "no-modules", "--out-dir"]).arg(&out).arg(&compiled));
        let mut wasm = fs::read(out.join(format!("{stem}_core_bg.wasm"))).unwrap();
        let glue = fs::read(out.join(format!("{stem}_core.js"))).unwrap();
        let mut section = Vec::new();
        leb(4, &mut section);
        section.extend(b"glue");
        section.extend(&glue);
        wasm.push(0); // section id 0: custom
        leb(section.len(), &mut wasm);
        wasm.extend(section);
        fs::write(&target, &wasm).unwrap();
        fs::write(&stamp_file, stamp).unwrap();
        println!("{stem}.wasm: {} KB ({} KB of it bindings), {profile} build", wasm.len() / 1024, glue.len() / 1024);
    }

    // A server: its own workspace (server/.cargo/config.toml, and any machine settings in a parent folder), in
    // `<plugin>/server` (Augmented View's) or `<plugin>/service` (Scholar's), built as `<plugin>-server` or `-service`.
    let kind = if plugin.join("service").exists() { "service" } else { "server" };
    let server = plugin.join(kind);
    let exe = format!("{name}-{kind}{}", std::env::consts::EXE_SUFFIX);
    if flag("--server") {
        let gpu: &[&str] = if cfg!(target_os = "macos") { &["--no-default-features", "--features", "metal"] } else { &[] };
        run(Command::new("cargo").current_dir(&server).args(["build", "--release"]).args(gpu));
        println!("server: {}", server.join("target/release").join(&exe).display());
    }
    if flag("--cpu-server") {
        run(Command::new("cargo").current_dir(&server).args(["build", "--release", "--no-default-features", "--target-dir", "target/cpu"]));
        println!("CPU server: {}", server.join("target/cpu/release").join(&exe).display());
    }

    let main_js = bundle(&plugin.join("main.js"), &plugin.join(format!("{stem}.wasm")));
    let inlined = main_js.contains("const WASM = \"");
    if let Some(i) = args.iter().position(|a| a == "--release") {
        let dir = args.get(i + 1).filter(|d| !d.starts_with("--")).cloned().unwrap_or_else(|| panic!("--release needs a folder"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(Path::new(&dir).join("main.js"), &main_js).unwrap();
        for f in ["manifest.json", "styles.css"] {
            fs::copy(plugin.join(f), Path::new(&dir).join(f)).unwrap();
        }
        println!("release assets in {dir}{}", if inlined { " (module inlined in main.js)" } else { "" });
    }
    if let Some(i) = args.iter().position(|a| a == "--deploy") {
        let env = format!("{}_DEPLOY", name.replace('-', "_").to_uppercase());
        let dir = args.get(i + 1).filter(|d| !d.starts_with("--")).cloned().or_else(|| std::env::var(&env).ok());
        let dir = dir.unwrap_or_else(|| panic!("--deploy needs the plugin's folder in a vault: --deploy <vault>/.obsidian/plugins/{name} (or {env})"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(Path::new(&dir).join("main.js"), &main_js).unwrap();
        for f in ["manifest.json", "styles.css"] {
            fs::copy(plugin.join(f), Path::new(&dir).join(f)).unwrap();
        }
        let wasm = format!("{stem}.wasm");
        if inlined {
            _ = fs::remove_file(Path::new(&dir).join(&wasm)); // an older deploy's
        } else {
            fs::copy(plugin.join(&wasm), Path::new(&dir).join(&wasm)).unwrap();
        }
        println!("copied to {dir}");
    }
}
