//! The hardware the models run on: what this computer has and whether this build can use its GPU (`probe`), which
//! device each model gets and why (every fallback is noted with its reason, so the plugin can say whether CUDA is in use
//! and what it fell back from), whether a model fits, how busy the GPU and the computer are, and the pacing that keeps
//! the GPU at or below the load limit. Moved here from Augmented View's server (2026-10-25) so Scholar's service uses
//! the same code; the device placement and pacing that need candle are in `device` (feature `candle`).
//!
//! References: candle's devices (<https://docs.rs/candle-core/latest/candle_core/enum.Device.html>), NVIDIA's management
//! library through nvml-wrapper (<https://docs.rs/nvml-wrapper>, <https://docs.nvidia.com/deploy/nvml-api/>), sysinfo
//! (<https://docs.rs/sysinfo>).

// Without candle, the helpers only `device` uses (free memory, the pacing gate) serve nothing.
#![cfg_attr(not(feature = "candle"), allow(dead_code))]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

#[cfg(feature = "candle")]
mod device;
#[cfg(feature = "candle")]
pub use device::{free_gpu_mb, paced, place};

pub const MB: u64 = 1 << 20;

/// Why the server runs other than fully on the GPU: reported in its ready line, /health and /stats.
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn note(s: String) {
    let mut n = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    if !n.contains(&s) {
        eprintln!("{s}");
        n.push(s);
    }
}

pub fn notes() -> Vec<String> {
    NOTES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The CUDA libraries this build delay-loads (build.rs) are present: loading one that is missing would stop the server.
fn cuda_libraries() -> Result<(), String> {
    #[cfg(feature = "cuda")]
    {
        let missing: Vec<&str> = env!("CUDA_DLLS").split(';').filter(|d| !d.is_empty() && unsafe { libloading::Library::new(d) }.is_err()).collect();
        if !missing.is_empty() {
            return Err(format!("CUDA's libraries are not installed ({}; https://developer.nvidia.com/cuda-downloads)", missing.join(", ")));
        }
    }
    Ok(())
}

fn available_ram_mb() -> u64 {
    sys(|s| {
        s.refresh_memory();
        s.available_memory() / MB
    })
}

fn sys<R>(f: impl FnOnce(&mut System) -> R) -> R {
    static SYS: Mutex<Option<System>> = Mutex::new(None);
    f(SYS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(System::new))
}

#[cfg(feature = "cuda")]
fn nvml() -> Option<&'static nvml_wrapper::Nvml> {
    static NVML: std::sync::OnceLock<Option<nvml_wrapper::Nvml>> = std::sync::OnceLock::new();
    NVML.get_or_init(|| nvml_wrapper::Nvml::init().ok()).as_ref()
}

#[cfg(feature = "cuda")]
fn gpu_name() -> Option<String> {
    nvml()?.device_by_index(0).ok()?.name().ok()
}

#[cfg(not(feature = "cuda"))]
fn gpu_name() -> Option<String> {
    None
}

/// GPU 0's memory and its other readings, from NVML (CUDA and NVML both number devices from the fastest by default;
/// with one GPU they are the same).
#[cfg(feature = "cuda")]
fn nvidia() -> Option<(nvml_wrapper::struct_wrappers::device::MemoryInfo, Value)> {
    use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
    let d = nvml()?.device_by_index(0).ok()?;
    let m = d.memory_info().ok()?;
    let v = json!({
        "name": d.name().ok(),
        "util": d.utilization_rates().ok().map(|u| u.gpu),
        "mem_used_mb": m.used / MB,
        "mem_total_mb": m.total / MB,
        "power_w": d.power_usage().ok().map(|p| p / 1000),
        "power_limit_w": d.enforced_power_limit().ok().map(|p| p / 1000),
        "temp_c": d.temperature(TemperatureSensor::Gpu).ok(),
    });
    Some((m, v))
}

#[cfg(not(feature = "cuda"))]
fn nvidia() -> Option<(MemoryInfo, Value)> {
    None
}

#[cfg(not(feature = "cuda"))]
pub struct MemoryInfo {
    #[allow(dead_code)] // read by device::free_gpu_mb (feature candle)
    free: u64,
}

/// The readings the plugin shows: the GPU's (all programs together, as the card reports them; null without an NVIDIA
/// GPU in use), the computer's CPU load and memory (all programs), and the server's own memory and share of the CPU.
pub fn stats(on_cuda: bool) -> Value {
    let gpu = if on_cuda { nvidia().map(|(_, v)| v) } else { None };
    let (ram, cpu, server) = sys(|s| {
        s.refresh_memory();
        s.refresh_cpu_usage(); // the change since the last call: /stats is polled every few seconds
        let pid = sysinfo::get_current_pid().ok();
        if let Some(pid) = pid {
            s.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_memory().with_cpu());
        }
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        let p = pid.and_then(|pid| s.process(pid));
        (
            json!({ "used_mb": (s.total_memory() - s.available_memory()) / MB, "total_mb": s.total_memory() / MB }),
            json!({ "util": s.global_cpu_usage().round() }),
            json!({ "ram_mb": p.map(|p| p.memory() / MB), "cpu": p.map(|p| (p.cpu_usage() / cores).round()) }),
        )
    });
    json!({ "gpu": gpu, "cpu": cpu, "ram": ram, "server": server, "gpu_limit": LIMIT.load(Ordering::Relaxed) })
}

static GATE: Mutex<()> = Mutex::new(());
static LIMIT: AtomicU32 = AtomicU32::new(100);

pub fn set_limit(percent: u32) {
    LIMIT.store(percent.clamp(10, 100), Ordering::Relaxed);
}

// ----- what the computer has, found without opening the GPU (no CUDA context, no GPU memory taken)

/// What `probe` found: the build, the GPU, the computer, and whether this build can use the GPU (the device it would
/// use) or why not.
#[derive(Clone, Debug)]
pub struct Hardware {
    /// "cuda", "metal" or "cpu": the features this server was built with.
    pub build: &'static str,
    pub gpu: Option<String>,
    pub gpu_mb: Option<u64>,
    /// The CUDA version the driver supports ("13.0") and the GPU's compute capability ("8.9"), NVIDIA only.
    pub cuda: Option<String>,
    pub compute: Option<String>,
    pub ram_mb: u64,
    pub cores: usize,
    pub usable: Result<String, String>,
}

impl Hardware {
    /// One line for the settings tab: "NVIDIA GeForce RTX 4060 Ti, 8 GB, CUDA 13.0, compute 8.9: compatible".
    pub fn summary(&self) -> String {
        let gb = |mb: u64| format!("{} GB", (mb as f64 / 1024.0).round());
        let what = [self.gpu.clone(), self.gpu_mb.map(gb), self.cuda.as_ref().map(|c| format!("CUDA {c}")), self.compute.as_ref().map(|c| format!("compute {c}"))];
        let what: Vec<String> = what.into_iter().flatten().collect();
        let computer = format!("{} cores, {} memory", self.cores, gb(self.ram_mb));
        let what = match (what.is_empty(), self.build) {
            (false, _) => what.join(", "),
            (true, "cpu") => format!("a CPU-only build, which does not look for a GPU; {computer}"),
            (true, _) => format!("no GPU found; {computer}"),
        };
        match &self.usable {
            Ok(_) => format!("{what}: compatible"),
            Err(why) => format!("{what}: not usable ({why})"),
        }
    }

    pub fn json(&self) -> Value {
        json!({ "build": self.build, "gpu": self.gpu, "gpu_mb": self.gpu_mb, "cuda": self.cuda, "compute": self.compute,
            "ram_mb": self.ram_mb, "cores": self.cores, "compatible": self.usable.is_ok(),
            "why": self.usable.as_ref().err(), "summary": self.summary() })
    }
}

/// This computer's hardware and whether this build can use its GPU. Nothing is opened on the GPU: NVIDIA's driver is
/// asked through NVML, CUDA's libraries are only looked for, and Apple's GPU is assumed on Apple silicon (Metal builds).
pub fn probe() -> Hardware {
    let build = if cfg!(feature = "cuda") { "cuda" } else if cfg!(feature = "metal") { "metal" } else { "cpu" };
    let ram_mb = sys(|s| {
        s.refresh_memory();
        s.total_memory() / MB
    });
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    #[cfg_attr(not(any(feature = "cuda", feature = "metal")), allow(unused_mut))] // filled in below for a GPU build
    let mut h = Hardware { build, gpu: None, gpu_mb: None, cuda: None, compute: None, ram_mb, cores, usable: Err("this build has no GPU support".into()) };
    #[cfg(feature = "cuda")]
    {
        h.usable = Err("no NVIDIA driver answered (NVML could not start)".into());
        if let Some(n) = nvml() {
            h.cuda = n.sys_cuda_driver_version().ok().map(|v| format!("{}.{}", nvml_wrapper::cuda_driver_version_major(v), nvml_wrapper::cuda_driver_version_minor(v)));
            if let Ok(d) = n.device_by_index(0) {
                h.gpu = d.name().ok();
                h.gpu_mb = d.memory_info().ok().map(|m| m.total / MB);
                h.compute = d.cuda_compute_capability().ok().map(|c| format!("{}.{}", c.major, c.minor));
                h.usable = cuda_libraries().map(|()| format!("CUDA, {}", gpu_name().unwrap_or_else(|| "GPU 0".into())));
            } else {
                h.usable = Err("the NVIDIA driver reports no GPU".into());
            }
        }
    }
    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    {
        let apple = cfg!(all(target_os = "macos", target_arch = "aarch64"));
        h.gpu = apple.then(|| "Apple GPU".into());
        h.gpu_mb = apple.then_some(ram_mb); // it shares the computer's memory
        h.usable = if apple { Ok("Metal".into()) } else { Err("Metal needs a Mac with Apple silicon".into()) };
    }
    h
}

/// Where models would run, given *Use the GPU*: the GPU this build can use, else the CPU with the reason noted (as
/// `place` notes it). For a server that has not loaded a model yet; a model itself is placed with `place`.
pub fn choose(hw: &Hardware, use_gpu: bool) -> String {
    match (&hw.usable, use_gpu) {
        (Ok(device), true) => device.clone(),
        (_, false) => {
            note("on the CPU: the GPU is turned off in the settings".into());
            "CPU".into()
        }
        (Err(why), true) => {
            note(format!("on the CPU: {why}"));
            "CPU".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_have_every_part() {
        let s = stats(false);
        assert!(s["gpu"].is_null(), "no GPU readings unless on CUDA");
        assert!(s["ram"]["total_mb"].as_u64().unwrap() > 0);
        assert!(s["cpu"]["util"].is_number());
        assert!(s["server"]["ram_mb"].as_u64().unwrap() > 0);
        assert!((10..=100).contains(&s["gpu_limit"].as_u64().unwrap()));
    }

    #[test]
    fn probe_and_choose() {
        let h = probe();
        assert!(h.ram_mb > 0 && h.cores > 0);
        if !cfg!(feature = "cuda") && !cfg!(feature = "metal") {
            assert_eq!(h.usable, Err("this build has no GPU support".into()));
            assert_eq!(choose(&h, true), "CPU");
            assert!(notes().iter().any(|n| n == "on the CPU: this build has no GPU support"));
        }
        assert_eq!(choose(&h, false), "CPU");
        assert!(notes().iter().any(|n| n == "on the CPU: the GPU is turned off in the settings"));
        let j = h.json();
        assert_eq!(j["compatible"], h.usable.is_ok());
        assert!(j["summary"].as_str().unwrap().contains(if h.usable.is_ok() { ": compatible" } else { ": not usable" }));
        println!("{}", h.summary());
    }

    #[test]
    fn limit_is_clamped() {
        set_limit(5);
        assert_eq!(LIMIT.load(Ordering::Relaxed), 10);
        set_limit(100);
    }
}
