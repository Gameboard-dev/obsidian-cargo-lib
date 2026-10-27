//! Placing a candle model on a device, with its fallbacks, and pacing its GPU work (feature `candle`).

use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use candle_core::Device;

use super::{available_ram_mb, cuda_libraries, gpu_name, note, nvidia, GATE, LIMIT, MB};

pub const GPU_MARGIN_MB: u64 = 300; // left free for the display, other programs and the models' working memory

/// The GPU, opened once: CUDA, else Metal; or why there is none.
fn gpu() -> &'static Result<(Device, String), String> {
    static GPU: OnceLock<Result<(Device, String), String>> = OnceLock::new();
    GPU.get_or_init(|| {
        let cuda = match cuda_libraries().and_then(|()| Device::new_cuda(0).map_err(|e| e.to_string())) {
            Ok(d) => return Ok((d, format!("CUDA, {}", gpu_name().unwrap_or_else(|| "GPU 0".into())))),
            Err(_) if !cfg!(feature = "cuda") => "this build has no CUDA support".to_string(),
            Err(e) => format!("CUDA could not start: {e}"),
        };
        match Device::new_metal(0) {
            Ok(d) => Ok((d, "Metal".into())),
            Err(_) if !cfg!(feature = "metal") => Err(cuda),
            Err(e) => Err(format!("{cuda}; Metal could not start ({e})")),
        }
    })
}

/// The device for a model that needs about `gpu_mb` of GPU memory, or `ram_mb` of the computer's on the CPU: the GPU
/// when it is allowed and has room, else the CPU when the computer has room, else an error saying what is missing.
pub fn place(what: &str, use_gpu: bool, gpu_mb: u64, ram_mb: u64) -> Result<(Device, String), String> {
    place_within(what, use_gpu, gpu_mb, ram_mb, None)
}

/// `place`, within a GPU memory budget: `budget_left`, the MB of the budget the service's loaded models leave (None: no
/// budget). A model that would pass it goes to the CPU, and the notes say why.
pub fn place_within(what: &str, use_gpu: bool, gpu_mb: u64, ram_mb: u64, budget_left: Option<u64>) -> Result<(Device, String), String> {
    if !use_gpu {
        note("on the CPU: the GPU is turned off in the settings".into());
    } else if let Some(left) = budget_left.filter(|&left| left < gpu_mb) {
        note(format!("{what} on the CPU: it needs about {gpu_mb} MB of GPU memory and the GPU memory budget in the settings has {left} MB left"));
    } else {
        match gpu() {
            Ok((d, name)) => match free_gpu_mb(d) {
                Some(free) if free < gpu_mb + GPU_MARGIN_MB => {
                    note(format!("{what} on the CPU: the GPU has {free} MB free and needs about {} MB", gpu_mb + GPU_MARGIN_MB))
                }
                _ => return Ok((d.clone(), name.clone())),
            },
            Err(why) => note(format!("on the CPU: {why}")),
        }
    }
    let free = available_ram_mb();
    if free < ram_mb {
        return Err(format!("{what} needs about {ram_mb} MB of memory and this computer has {free} MB available"));
    }
    Ok((Device::Cpu, "CPU".into()))
}

/// Free memory of the device the models would use, in MB: NVIDIA's from NVML; Apple's GPU shares the computer's.
pub fn free_gpu_mb(d: &Device) -> Option<u64> {
    match d {
        Device::Cuda(_) => nvidia().map(|(m, _)| m.free / MB),
        Device::Metal(_) => Some(available_ram_mb()),
        Device::Cpu => None,
    }
}

/// One piece of GPU work (a model's forward pass and the read-back that waits for it): one at a time, so the two models
/// never load the GPU together, then a rest so the GPU works at most the load limit's share of the time (at 50%, as
/// long resting as working). Lower power on average means less heat, fan and coil noise; work on the CPU is not paced.
pub fn paced<R>(device: &Device, f: impl FnOnce() -> R) -> R {
    paced_at(device, LIMIT.load(Ordering::Relaxed), f)
}

/// Make the device's CUDA context this thread's current one, before anything touches the GPU on it (a tensor made, a
/// cache cleared): a request may run on any of the server's threads, and one without the context fails with
/// CUDA_ERROR_INVALID_CONTEXT (user, 2026-10-27: seen in the Search pane while the service ran on CUDA).
pub fn bind(device: &Device) {
    #[cfg(feature = "cuda")]
    if let Device::Cuda(d) = device {
        _ = d.cuda_stream().context().bind_to_thread();
    }
    #[cfg(not(feature = "cuda"))]
    let _ = device;
}

/// As `paced`, with a load limit of its own (work in the background, such as indexing, kept gentler than a search the
/// user waits for).
pub fn paced_at<R>(device: &Device, limit: u32, f: impl FnOnce() -> R) -> R {
    if device.is_cpu() {
        return f();
    }
    let _one = GATE.lock().unwrap_or_else(|e| e.into_inner());
    // the work may run on any of the server's threads: make the device's CUDA context this thread's current one first
    // (cudarc binds it before most calls; this covers the rest, where a thread without it fails with
    // CUDA_ERROR_INVALID_CONTEXT)
    #[cfg(feature = "cuda")]
    if let Device::Cuda(d) = device {
        _ = d.cuda_stream().context().bind_to_thread();
    }
    let t = Instant::now();
    let r = f();
    drop(_one);
    let limit = limit.clamp(10, 100);
    if limit < 100 {
        std::thread::sleep(t.elapsed() * (100 - limit) / limit);
    }
    r
}

/// Free memory of the GPU the models would use, in MB, or None without one (to choose between a model's sizes before
/// placing it).
pub fn gpu_free_mb() -> Option<u64> {
    gpu().as_ref().ok().and_then(|(d, _)| free_gpu_mb(d))
}
