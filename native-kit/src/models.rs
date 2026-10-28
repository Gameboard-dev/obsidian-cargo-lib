//! Model files: from the local Hugging Face cache when they are there
//! (<https://huggingface.co/docs/huggingface_hub/guides/manage-cache>), else from the server's own cache folder. Nothing
//! here downloads: Obsidian's developer policies forbid a plugin that installs its own dependencies, so a model file
//! that is on neither disk is an error that names it (`MISSING`), and the server downloads model files only when the
//! user runs its download command (Quarry: `quarry-service --download all`, its service's model_files.rs; Augmented
//! View: `augmented-view-server --download all`, with `download`), into `ours`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type TokenSource = Box<dyn Fn() -> Option<String> + Send + Sync>;
static TOKEN: OnceLock<TokenSource> = OnceLock::new();

/// The start of the error for a model file that is on neither disk.
pub const MISSING: &str = "not downloaded:";

/// Where a Hugging Face token comes from, for gated models: the server sets it once (Quarry's from its keychain), and
/// it is asked at each download the user starts, sent as `Authorization: Bearer` and never kept or logged.
pub fn token_from(source: impl Fn() -> Option<String> + Send + Sync + 'static) {
    _ = TOKEN.set(Box::new(source));
}

/// The Hugging Face token, when one is set and not blank: the server's own source (`token_from`), else the environment
/// variable `HF_TOKEN`, as Hugging Face's own tools read it
/// (<https://huggingface.co/docs/huggingface_hub/package_reference/environment_variables#hftoken>).
pub fn token() -> Option<String> {
    let set = TOKEN.get().and_then(|f| f()).or_else(|| std::env::var("HF_TOKEN").ok());
    set.filter(|t| !t.trim().is_empty())
}

/// What to say when the Hub refuses a download (401 or 403): the model is likely gated.
pub fn refused(what: &str, status: u16, sent_token: bool) -> String {
    let fix = if sent_token {
        "the Hugging Face token (the environment variable HF_TOKEN) was not accepted: check it, and that you accepted the model's terms on its Hugging Face page"
    } else {
        "the model may need a Hugging Face token: accept its terms on its Hugging Face page, make a read token (https://huggingface.co/settings/tokens), and set it as the environment variable HF_TOKEN in the terminal before running the download command again"
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

/// A model file already on disk: the local Hugging Face cache, else the server's own folder.
pub fn local(cache: &Path, repo: &str, file: &str) -> Option<PathBuf> {
    hf_cache(repo, file).or_else(|| Some(ours(cache, repo, file)).filter(|p| p.exists()))
}

/// A model file on disk, or the `MISSING` error naming it. Never downloads.
pub fn need(cache: &Path, repo: &str, file: &str) -> Result<PathBuf, String> {
    local(cache, repo, file).ok_or_else(|| format!("{MISSING} {repo} {file}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_file_is_an_error_and_nothing_is_fetched() {
        let cache = std::env::temp_dir().join(format!("native-kit-models-{}", std::process::id()));
        let e = super::need(&cache, "nobody/no-such-model", "config.json").unwrap_err();
        assert!(e.starts_with(super::MISSING) && e.contains("nobody/no-such-model config.json"), "{e}");
        assert!(!cache.exists(), "nothing was written");
    }
}
