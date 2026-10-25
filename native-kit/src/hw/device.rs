//! Placing a candle model on a device, with its fallbacks, and pacing its GPU work (feature `candle`).

use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use candle_core::Device;

use super::{available_ram_mb, cuda_libraries, gpu_name, note, nvidia, GATE, LIMIT, MB};

const GPU_MARGIN_MB: u64 = 300; // left free for the display, other programs and the models' working memory

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
    if !use_gpu {
        note("on the CPU: the GPU is turned off in the settings".into());
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
    if device.is_cpu() {
        return f();
    }
    let _one = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let t = Instant::now();
    let r = f();
    let limit = LIMIT.load(Ordering::Relaxed);
    if limit < 100 {
        std::thread::sleep(t.elapsed() * (100 - limit) / limit);
    }
    r
}
