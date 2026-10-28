//! Model files: from the local Hugging Face cache when they are there
//! (<https://huggingface.co/docs/huggingface_hub/guides/manage-cache>), else from the server's own cache folder,
//! downloaded into it first (from the Hub's `resolve` URLs, <https://huggingface.co/docs/hub/models-downloading>), with
//! the operating system's certificate store (antivirus and company TLS inspection re-sign connections). Progress goes to
//! stdout as `{"progress": ...}` lines, which the plugins show. The same rules as Augmented View's server (its
//! models.rs, which can move here).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;

type TokenSource = Box<dyn Fn() -> Option<String> + Send + Sync>;
static TOKEN: OnceLock<TokenSource> = OnceLock::new();

/// Where a Hugging Face token comes from, for gated models: the server sets it once (Quarry's from its keychain), and
/// it is asked at each download, sent as `Authorization: Bearer` and never kept or logged. Unset, downloads go without.
pub fn token_from(source: impl Fn() -> Option<String> + Send + Sync + 'static) {
    _ = TOKEN.set(Box::new(source));
}

fn token() -> Option<String> {
    TOKEN.get().and_then(|f| f()).filter(|t| !t.trim().is_empty())
}

/// What to say when the Hub refuses a download (401 or 403): the model is likely gated.
fn refused(what: &str, status: u16, sent_token: bool) -> String {
    let fix = if sent_token {
        "the Hugging Face token set in the settings (Keys → Hugging Face) was not accepted: check it, and that you accepted the model's terms on its Hugging Face page"
    } else {
        "the model may need a Hugging Face token: accept its terms on its Hugging Face page, then set a read token (https://huggingface.co/settings/tokens) in the settings (Keys → Hugging Face)"
    };
    format!("downloading {what}: Hugging Face refused it (HTTP {status}); {fix}")
}

/// The default cache folder for a server named `name`: the operating system's per-user cache (Windows `%LOCALAPPDATA%`,
/// macOS `~/Library/Caches`, elsewhere `$XDG_CACHE_HOME` or `~/.cache`).
pub fn default_cache(name: &str) -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join(name)
}

/// A file of a model in the local Hugging Face cache: `{HF_HUB_CACHE or HF_HOME/hub}/models--{org}--{name}/snapshots/
/// {revision of refs/main}/{file}`.
pub fn hf_cache(repo: &str, file: &str) -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| h.join(".cache/huggingface"));
    let hub = env("HF_HUB_CACHE").or_else(|| env("HF_HOME").or(home).map(|h| h.join("hub")))?;
    let dir = hub.join(format!("models--{}", repo.replace('/', "--")));
    let rev = std::fs::read_to_string(dir.join("refs/main")).ok()?;
    Some(dir.join("snapshots").join(rev.trim()).join(file)).filter(|p| p.exists())
}

/// Where a server keeps a model file it downloaded, under its cache folder.
pub fn ours(cache: &Path, repo: &str, file: &str) -> PathBuf {
    cache.join("models").join(repo.replace('/', "--")).join(file)
}

/// A model file, downloaded first when neither cache has it.
pub fn need(cache: &Path, repo: &str, file: &str) -> Result<PathBuf, String> {
    if let Some(p) = hf_cache(repo, file) {
        return Ok(p);
    }
    let p = ours(cache, repo, file);
    if !p.exists() {
        download(&format!("https://huggingface.co/{repo}/resolve/main/{file}"), &p, &format!("{repo} {file}"))?;
    }
    Ok(p)
}

/// Download `url` to `dest` (through a `.part` file, so an interrupted download is never taken for a whole one).
pub fn download(url: &str, dest: &Path, what: &str) -> Result<(), String> {
    let e = |x: std::io::Error| format!("downloading {what}: {x}");
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder().tls_config(tls).build().into();
    // the token goes to huggingface.co only: ureq drops the header on a redirect to the file's CDN
    let token = token();
    let mut req = agent.get(url);
    if let Some(t) = &token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let mut res = req.call().map_err(|x| match x {
        ureq::Error::StatusCode(s @ (401 | 403)) => refused(what, s, token.is_some()),
        x => format!("downloading {what}: {x}"),
    })?;
    let size = res.body().content_length();
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new("."))).map_err(e)?;
    let part = dest.with_extension("part");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&part).map_err(e)?);
    let (mut body, mut buf, mut done, mut shown) = (res.body_mut().as_reader(), vec![0u8; 1 << 20], 0u64, u64::MAX);
    loop {
        let n = body.read(&mut buf).map_err(e)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(e)?;
        done += n as u64;
        let pct = size.map_or(0, |s| done * 100 / s.max(1));
        if pct != shown && (pct % 5 == 0 || shown == u64::MAX) {
            shown = pct;
            let total = size.map_or(String::new(), |s| format!(" of {}", s >> 20));
            println!("{}", json!({ "progress": format!("downloading {what}: {}{total} MB", done >> 20) }));
        }
    }
    out.flush().map_err(e)?;
    drop(out);
    if size.is_some_and(|s| s != done) {
        return Err(format!("downloading {what}: the connection closed after {done} bytes"));
    }
    std::fs::rename(part, dest).map_err(e)
}
