//! Which of the plugins' services hold GPU memory, so that one which needs the GPU can say how much is held and, when
//! the user agrees, have the others let it go (user, 2026-10-27: the GPU is shared by turns, and the messages name no
//! plugin). Each service that places a model on the GPU writes a claim, one small JSON file per service in a folder of
//! the user's local application data (`<local data>/obsidian-kit/gpu/<service>.json`: its port, process id, the MB it
//! holds and the path that frees it), and removes it when it lets the memory go or stops. A claim whose process is
//! gone is ignored and removed. Releasing is an HTTP POST to `127.0.0.1:<port><path>` of that service: nothing is run
//! and no address comes from anywhere but these files, which only the user's own services write.

use std::path::PathBuf;

use serde_json::{json, Value};

/// One service's hold on the GPU.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub service: String,
    pub port: u16,
    pub pid: u32,
    pub mb: u64,
    /// The path that frees the memory (`/gpu/release`, or `/shutdown` for a service that holds nothing else).
    pub release: String,
}

fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("obsidian-kit").join("gpu"))
}

/// A service name as a file name: letters, digits and `-` only.
fn file_of(service: &str) -> Option<PathBuf> {
    let ok = !service.is_empty() && service.len() <= 40 && service.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then(|| dir().map(|d| d.join(format!("{service}.json")))).flatten()
}

/// This service holds `mb` of GPU memory, freed by POST `release` on `port`.
pub fn hold(service: &str, port: u16, mb: u64, release: &str) {
    let Some(f) = file_of(service) else { return };
    if let Some(d) = f.parent() {
        _ = std::fs::create_dir_all(d);
    }
    let v = json!({ "service": service, "port": port, "pid": std::process::id(), "mb": mb, "release": release });
    _ = std::fs::write(f, v.to_string());
}

/// This service holds nothing on the GPU any more.
pub fn drop_claim(service: &str) {
    if let Some(f) = file_of(service) {
        _ = std::fs::remove_file(f);
    }
}

fn alive(pid: u32) -> bool {
    super::sys(|s| {
        let p = sysinfo::Pid::from_u32(pid);
        s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
        s.process(p).is_some()
    })
}

fn parse(v: &Value) -> Option<Claim> {
    let release = v["release"].as_str().unwrap_or("/shutdown");
    // only a plain path on the service itself
    if !release.starts_with('/') || release.contains("//") || release.contains("..") || release.len() > 40 {
        return None;
    }
    Some(Claim {
        service: v["service"].as_str()?.to_string(),
        port: u16::try_from(v["port"].as_u64()?).ok().filter(|p| *p > 0)?,
        pid: u32::try_from(v["pid"].as_u64()?).ok()?,
        mb: v["mb"].as_u64().unwrap_or(0),
        release: release.to_string(),
    })
}

/// The other services' live claims (a stale one, whose process is gone, is removed).
pub fn others(service: &str) -> Vec<Claim> {
    let Some(d) = dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let Some(c) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| parse(&v)) else { continue };
        if c.service == service || c.pid == std::process::id() {
            continue;
        }
        if !alive(c.pid) {
            _ = std::fs::remove_file(&p);
            continue;
        }
        out.push(c);
    }
    out
}

/// What the plugin is told when the GPU has no room: how much is needed, free, and held by the others (no names).
pub fn shortage(service: &str, what: &str, need_mb: u64) -> Option<Value> {
    let free = super::device_free_mb()?;
    if free >= need_mb + super::GPU_MARGIN_MB {
        return None;
    }
    let others = others(service);
    let held: u64 = others.iter().map(|c| c.mb).sum();
    // the user's own plugins by name (user, 2026-10-27); anything else that holds memory is not named
    let holders: Vec<&str> = others.iter().map(|c| display_name(&c.service)).collect();
    Some(json!({ "what": what, "need_mb": need_mb + super::GPU_MARGIN_MB, "free_mb": free, "others_mb": held, "others": others.len(), "holders": holders }))
}

/// What a service does before loading models that need `need` MB of GPU memory, by the user's answer (`mode`): every
/// GPU service of the workspace decides this way (user, 2026-10-27).
#[derive(Debug, PartialEq)]
pub enum Room {
    /// Load on the GPU (there is room, or the user's other services were asked to free theirs).
    Gpu,
    /// The user chose the CPU.
    Cpu,
    /// Not enough room and no answer yet: the plugin asks the user (the shortage, as `shortage` gives it).
    Ask(Value),
}

/// `mode`: "free" (have the user's other GPU services let go, then the GPU), "cpu", or anything else (ask when short).
pub fn room(service: &str, what: &str, need_mb: u64, mode: &str) -> Room {
    if need_mb == 0 {
        return Room::Gpu;
    }
    match mode {
        "cpu" => Room::Cpu,
        "free" => {
            if !release_others(service, need_mb, std::time::Duration::from_secs(20)) {
                eprintln!("{service}: the GPU still has too little free memory after asking the other services; {what} load where they fit");
            }
            Room::Gpu
        }
        _ => shortage(service, what, need_mb).map_or(Room::Gpu, Room::Ask),
    }
}

/// A service's plugin as the user knows it.
pub fn display_name(service: &str) -> &str {
    match service {
        "scholar" => "Scholar",
        "augmented-view" => "Augmented View",
        s => s,
    }
}

/// Asks every other service holding GPU memory to let it go (POST to its own release path on 127.0.0.1), then waits
/// up to `wait` for `need_mb` (plus the margin) to be free. Whether it is.
pub fn release_others(service: &str, need_mb: u64, wait: std::time::Duration) -> bool {
    for c in others(service) {
        let url = format!("http://127.0.0.1:{}{}", c.port, c.release);
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(5))).build().into();
        match agent.post(&url).send_empty() {
            Ok(_) => super::note(format!("asked a service on port {} to free its {} MB of GPU memory", c.port, c.mb)),
            Err(e) => super::note(format!("a service on port {} did not free its GPU memory: {e}", c.port)),
        }
    }
    let t = std::time::Instant::now();
    loop {
        if super::device_free_mb().is_some_and(|f| f >= need_mb + super::GPU_MARGIN_MB) {
            return true;
        }
        if t.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_checked() {
        assert!(file_of("scholar").is_some() && file_of("../x").is_none() && file_of("a b").is_none());
        let ok = json!({ "service": "x", "port": 8766, "pid": 1, "mb": 900, "release": "/gpu/release" });
        assert_eq!(parse(&ok).map(|c| c.release), Some("/gpu/release".into()));
        for bad in ["http://evil/", "//evil", "/../x", "x"] {
            assert!(parse(&json!({ "service": "x", "port": 1, "pid": 1, "release": bad })).is_none(), "{bad}");
        }
        assert!(parse(&json!({ "service": "x", "port": 70000, "pid": 1 })).is_none());
    }
}
