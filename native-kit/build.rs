//! Windows, CUDA build: candle links cuBLAS and cuRAND at load time (<https://docs.rs/cudarc>, `dynamic-linking`), so
//! without CUDA installed Windows refuses to start a server at all, with an error box. The server delay-loads them
//! (<https://learn.microsoft.com/en-us/cpp/build/reference/linker-support-for-delay-loaded-dlls>) so it can start, look
//! for them first (hw.rs) and use the CPU, with the reason, where they are missing. The libraries' names carry CUDA's
//! major version, read from nvcc.
//!
//! This script sets `CUDA_DLLS` (the libraries hw.rs checks for) and, with candle, tells the server's own build script
//! which to delay-load (`cargo:delayload`, read there as DEP_NATIVE_KIT_DELAYLOAD), since only the binary can link them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|o| o == "windows");
    if !windows || std::env::var_os("CARGO_FEATURE_CUDA").is_none() {
        println!("cargo:rustc-env=CUDA_DLLS=");
        return;
    }
    let out = std::process::Command::new("nvcc").arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let major = out.split("release ").nth(1).and_then(|r| r.split('.').next()).unwrap_or("13").to_string();
    let linked = [format!("cublas64_{major}.dll"), "curand64_10.dll".into()];
    let dlls = [linked[0].clone(), format!("cublasLt64_{major}.dll"), linked[1].clone()]; // cuBLAS loads cuBLASLt itself
    println!("cargo:rustc-env=CUDA_DLLS={}", dlls.join(";"));
    if std::env::var_os("CARGO_FEATURE_CANDLE").is_some() {
        println!("cargo:delayload={}", linked.join(";"));
    }
}
