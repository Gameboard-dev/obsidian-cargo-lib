//! `cargo run -p pack --release -- <plugin> [--fast] [--server] [--cpu-server] [--deploy DIR]`, from this folder's
//! parent: builds a plugin's crate (`<plugin>/core`) to WebAssembly, generates its JavaScript bindings with
//! wasm-bindgen, writes them into the main.js it deploys or releases (no eval at run time) and keeps them in the .wasm as a
//! custom section named "glue" too (for a worker's script), so main.js stays the only JavaScript
//! file. Writes `<plugin>/<plugin_with_underscores>.wasm` (augmented_view.wasm, dictionary_and_synonyms.wasm).
//!
//! - `--fast` builds with the `fast` profile (thin LTO, incremental; ../Cargo.toml) instead of `release`.
//! - `--server` also builds the plugin's server (`<plugin>/server`, Augmented View's; `<plugin>/service`, Quarry's) for this
//!   computer: CUDA, or Metal on macOS.
//! - `--cpu-server` also builds its CPU-only version, into server/target/cpu (Augmented View's fallback, `cpuServer`).
//! - `--deploy DIR` copies the plugin into DIR, a vault's `.obsidian/plugins/<plugin>` (or the folder in the environment
//!   variable `<PLUGIN>_DEPLOY`, e.g. AUGMENTED_VIEW_DEPLOY). Servers are not copied: point the plugin's settings at them.
//! - `--release DIR` writes the release's assets into DIR: main.js, manifest.json and styles.css, the only files
//!   Obsidian installs from a release. A main.js with the line `const WASM = null;` gets the module there, base64-encoded
//!   (so does a deployed one), and the .wasm is then not needed beside it. A built program (Quarry's service) is
//!   released with its SHA-256 and its licences (the plugin's LICENSE and `licences/<kind>.txt`); without them the
//!   release fails.
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
/// The source main.js evaluates the bindings it reads from the module (`new Function`, fine while developing); the
/// main.js that is deployed or released has them written in (`glue`, wasm-bindgen's no-modules script, which defines
/// `wasm_bindgen`), so a plugin evaluates no code from a string at run time (user, 2026-10-27: no eval). The custom
/// section stays in the .wasm for a worker's script (Augmented View's, Dictionary's), which starts from a Blob.
const EVAL_GLUE: &str = "new Function(`${glue}\\nreturn wasm_bindgen;`)()";

fn inline_glue(js: &str, glue: &str) -> String {
    if !js.contains(EVAL_GLUE) {
        return js.to_string();
    }
    let at = js.find("module.exports").expect("main.js exports its plugin class");
    let mut out = String::with_capacity(js.len() + glue.len() + 200);
    out.push_str(&js[..at]);
    out.push_str("// wasm-bindgen's bindings for the .wasm, written in by pack (they define wasm_bindgen)\n");
    out.push_str(glue.trim_end());
    out.push_str("\n\n");
    out.push_str(&js[at..].replacen(EVAL_GLUE, "wasm_bindgen", 1));
    out
}

fn bundle(main_js: &Path, wasm: &Path, glue: &str) -> String {
    let js = inline_glue(&fs::read_to_string(main_js).unwrap(), glue);
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
    // `<plugin>/server` (Augmented View's) or `<plugin>/service` (Quarry's), built as `<plugin>-server` or `-service`.
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

    let glue = fs::read_to_string(out.join(format!("{stem}_core.js"))).expect("the bindings wasm-bindgen wrote");
    let main_js = bundle(&plugin.join("main.js"), &plugin.join(format!("{stem}.wasm")), &glue);
    let inlined = main_js.contains("const WASM = \"");
    if let Some(i) = args.iter().position(|a| a == "--release") {
        let dir = args.get(i + 1).filter(|d| !d.starts_with("--")).cloned().unwrap_or_else(|| panic!("--release needs a folder"));
        // A plugin's program (Quarry's service), as its settings tab downloads it: `<plugin>-<kind>-<platform>[.exe]` and
        // its `.sha256`, for a release named `<plugin>-<kind>-v<version>`.
        let built = server.join("target/release").join(&exe);
        // The program's licences go with it, or there is no release (checked before anything is written): the plugin's
        // LICENSE and the third-party licences of the program's crates and code, `<plugin>/licences/<kind>.txt`
        // (Quarry's licences/generate.py writes them).
        let licences = [
            (plugin.join("LICENSE"), format!("{name}-{kind}-LICENSE.txt")),
            (plugin.join("licences").join(format!("{kind}.txt")), format!("{name}-{kind}-THIRD-PARTY-LICENSES.txt")),
        ];
        if built.exists() {
            for (from, _) in &licences {
                assert!(from.exists(), "no release: {} is missing, and the {kind} cannot ship without its licences (Quarry: python licences/generate.py)", from.display());
            }
        }
        fs::create_dir_all(&dir).unwrap();
        fs::write(Path::new(&dir).join("main.js"), &main_js).unwrap();
        for f in ["manifest.json", "styles.css"] {
            fs::copy(plugin.join(f), Path::new(&dir).join(f)).unwrap();
        }
        if built.exists() {
            for (from, to) in &licences {
                fs::copy(from, Path::new(&dir).join(to)).unwrap();
                println!("{to}");
            }
            use sha2::Digest;
            let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
                ("windows", "x86_64") => "windows-x64",
                ("linux", "x86_64") => "linux-x64",
                ("macos", "aarch64") => "macos-arm64",
                ("macos", "x86_64") => "macos-x64",
                (os, arch) => panic!("no release name for {os} {arch}"),
            };
            let asset = format!("{name}-{kind}-{platform}{}", std::env::consts::EXE_SUFFIX);
            let bytes = fs::read(&built).unwrap();
            let hash: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
            fs::write(Path::new(&dir).join(&asset), &bytes).unwrap();
            fs::write(Path::new(&dir).join(format!("{asset}.sha256")), format!("{hash}  {asset}\n")).unwrap();
            println!("{asset}: SHA-256 {hash} (publish both under the release {name}-{kind}-v<version>)");
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
