//! Downloading a model file, for a server's download command only: the user runs it in a terminal (Augmented View:
//! `augmented-view-server --download all`), never the plugin (Obsidian's developer policies forbid a plugin that
//! installs its own dependencies; `models` finds the files once they are on disk).
//!
//! A file goes through a `.part` file beside its destination, resumed with an HTTP `Range` request when an earlier run
//! was interrupted (Ctrl+C, a lost connection), its size and checksum checked before it is renamed into place: SHA-256
//! for files stored with Git LFS, the Git blob SHA-1 for the small ones kept in Git itself, as the Hugging Face API
//! gives them (`https://huggingface.co/api/models/<repo>?blobs=true`). A download can be cancelled between reads; its
//! `.part` is kept for the next run. A Hugging Face token (`models::token`) is sent to huggingface.co only.
//!
//! References: the Hub's file URLs (<https://huggingface.co/docs/hub/models-downloading>), its API
//! (<https://huggingface.co/docs/hub/api>), HTTP range requests (<https://www.rfc-editor.org/rfc/rfc9110#name-range-requests>),
//! Git's blob hashing (<https://git-scm.com/book/en/v2/Git-Internals-Git-Objects>).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha1::Digest as _;

/// The Hugging Face Hub.
pub const HUB: &str = "https://huggingface.co";

/// The error a cancelled download ends with.
pub const CANCELLED: &str = "cancelled";

/// One file to download, with its size (0: unknown) and checksum (none: not checked).
#[derive(Clone, Debug, PartialEq)]
pub struct FileSpec {
    pub name: String,
    pub bytes: u64,
    /// SHA-256 of the file (Git LFS files).
    pub sha256: Option<String>,
    /// The Git blob SHA-1 (`blob <size>\0<content>`), for the small files kept in Git itself.
    pub git_sha1: Option<String>,
}

/// The URL of a file of a Hugging Face model at a revision (a commit, or "main").
pub fn hub_url(hub: &str, repo: &str, revision: &str, file: &str) -> String {
    format!("{hub}/{repo}/resolve/{revision}/{file}")
}

/// The `.part` file a download writes before the file is whole and checked.
pub fn part_of(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// An HTTP client with the operating system's certificate store (antivirus and company TLS inspection re-sign
/// connections, <https://docs.rs/ureq/latest/ureq/tls/enum.RootCerts.html>).
pub fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).timeout_connect(Some(Duration::from_secs(30))).build().into()
}

/// Whether a URL is the Hugging Face Hub's, the only host a token is sent to.
fn is_hub(url: &str) -> bool {
    url.strip_prefix(HUB).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn get(agent: &ureq::Agent, url: &str, range_from: u64, what: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    // uncompressed, so a file's length and range are its own; the token only to the Hub (ureq drops the header on a
    // redirect to the file's CDN)
    let mut req = agent.get(url).header("Accept-Encoding", "identity");
    let token = if is_hub(url) { crate::models::token() } else { None };
    if let Some(t) = &token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if range_from > 0 {
        req = req.header("Range", format!("bytes={range_from}-"));
    }
    req.call().map_err(|x| match x {
        ureq::Error::StatusCode(s @ (401 | 403)) => crate::models::refused(what, s, token.is_some()),
        ureq::Error::StatusCode(416) => "416".to_string(),
        x => format!("downloading {what}: {x}"),
    })
}

/// The files of a Hugging Face model that `wanted` keeps, with their sizes and checksums, from the Hub's API (for a
/// model set by hand, whose files no manifest lists).
pub fn list_files(agent: &ureq::Agent, hub: &str, repo: &str, wanted: &dyn Fn(&str) -> bool) -> Result<Vec<FileSpec>, String> {
    let what = format!("the file list of {repo}");
    let mut res = get(agent, &format!("{hub}/api/models/{repo}?blobs=true"), 0, &what)?;
    let text = res.body_mut().read_to_string().map_err(|e| format!("reading {what}: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("reading {what}: {e}"))?;
    let spec = |s: &Value| FileSpec {
        name: s["rfilename"].as_str().unwrap_or_default().to_string(),
        bytes: s["size"].as_u64().unwrap_or(0),
        sha256: s["lfs"]["sha256"].as_str().map(String::from),
        git_sha1: s["blobId"].as_str().filter(|_| s["lfs"].is_null()).map(String::from),
    };
    Ok(v["siblings"].as_array().into_iter().flatten().map(spec).filter(|f| wanted(&f.name)).collect())
}

/// The checksum of a file being written.
enum Hasher {
    Sha256(sha2::Sha256),
    GitSha1(sha1::Sha1),
    None,
}

impl Hasher {
    fn for_file(f: &FileSpec) -> Hasher {
        if f.sha256.is_some() {
            Hasher::Sha256(sha2::Sha256::new())
        } else if f.git_sha1.is_some() {
            let mut h = sha1::Sha1::new();
            h.update(format!("blob {}\0", f.bytes).as_bytes());
            Hasher::GitSha1(h)
        } else {
            Hasher::None
        }
    }

    fn update(&mut self, b: &[u8]) {
        match self {
            Hasher::Sha256(h) => h.update(b),
            Hasher::GitSha1(h) => h.update(b),
            Hasher::None => {}
        }
    }

    /// Ok when the file's checksum is the one expected (or none is known).
    fn check(self, f: &FileSpec) -> Result<(), String> {
        let (got, want) = match self {
            Hasher::Sha256(h) => (hex(&h.finalize()), f.sha256.clone().unwrap_or_default()),
            Hasher::GitSha1(h) => (hex(&h.finalize()), f.git_sha1.clone().unwrap_or_default()),
            Hasher::None => return Ok(()),
        };
        if got.eq_ignore_ascii_case(&want) {
            Ok(())
        } else {
            Err(format!("{}: the checksum does not match (expected {want}, got {got}); the download was deleted, so the next run starts it afresh", f.name))
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One file fetched from `url` to `dest` through `<dest>.part`: resumed from the `.part` when the server takes a
/// range, its size and checksum checked, then renamed into place. `on(bytes of this file so far)` reports progress;
/// `cancel` stops it between reads with [`CANCELLED`], keeping the `.part`.
pub fn fetch(agent: &ureq::Agent, url: &str, dest: &Path, f: &FileSpec, cancel: &AtomicBool, what: &str, on: &mut dyn FnMut(u64)) -> Result<(), String> {
    let e = |x: std::io::Error| format!("downloading {what}: {x}");
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new("."))).map_err(e)?;
    let part = part_of(dest);
    let mut have = std::fs::metadata(&part).map_or(0, |m| m.len());
    if f.bytes > 0 && have >= f.bytes {
        have = 0; // whole but never checked and renamed (or longer than it should be): read again from the start
    }
    let mut res = match get(agent, url, have, what) {
        Err(x) if x == "416" => {
            have = 0;
            get(agent, url, 0, what)?
        }
        r => r?,
    };
    let resumed = have > 0 && res.status().as_u16() == 206;
    if !resumed {
        have = 0;
    }
    let mut hasher = Hasher::for_file(f);
    if resumed {
        // the checksum covers the whole file, so the part already on disk is read into it first
        let mut old = std::fs::File::open(&part).map_err(e)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut left = have;
        while left > 0 {
            let n = old.read(&mut buf[..(left.min(1 << 20) as usize)]).map_err(e)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            left -= n as u64;
        }
    }
    let file = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).map_err(e)?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
    let (mut body, mut buf, mut done) = (res.body_mut().as_reader(), vec![0u8; 1 << 20], have);
    on(done);
    loop {
        if cancel.load(Ordering::SeqCst) {
            out.flush().map_err(e)?;
            return Err(CANCELLED.into());
        }
        let n = body.read(&mut buf).map_err(e)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(e)?;
        hasher.update(&buf[..n]);
        done += n as u64;
        on(done);
    }
    out.flush().map_err(e)?;
    drop(out);
    if f.bytes > 0 && done != f.bytes {
        return Err(format!("downloading {what}: the connection closed after {done} of {} bytes; run the download again to resume it", f.bytes));
    }
    if let Err(x) = hasher.check(f) {
        _ = std::fs::remove_file(&part);
        return Err(format!("downloading {what}: {x}"));
    }
    std::fs::rename(&part, dest).map_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("native-kit-download-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake server on 127.0.0.1: answers every GET with `body`, honouring `Range: bytes=N-` with 206 unless `ranges`
    /// is off; counts the requests and records whether one carried an Authorization header. No real network. Bodies
    /// stay under 64 KB (an antivirus that inspects loopback HTTP may hold larger plain-HTTP answers back).
    fn fake(body: Vec<u8>, ranges: bool) -> (String, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (hits, auth) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (h, a) = (hits.clone(), auth.clone());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                h.fetch_add(1, Ordering::SeqCst);
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut from = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("range: bytes=") {
                        from = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        a.store(true, Ordering::SeqCst);
                    }
                }
                let mut s = s;
                let (status, slice) = if from > 0 && ranges { ("206 Partial Content", &body[from..]) } else { ("200 OK", &body[..]) };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
                _ = s.write_all(head.as_bytes());
                _ = s.write_all(slice);
                // a graceful close (Windows resets a socket closed with anything unread)
                _ = s.flush();
                _ = s.shutdown(std::net::Shutdown::Write);
                _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                _ = std::io::copy(&mut r, &mut std::io::sink());
            }
        });
        (url, hits, auth)
    }

    fn spec(body: &[u8]) -> FileSpec {
        FileSpec { name: "model.safetensors".into(), bytes: body.len() as u64, sha256: Some(hex(&sha2::Sha256::digest(body))), git_sha1: None }
    }

    /// A download: the file whole, its checksum checked, the `.part` gone; no token sent to a host that is not the Hub.
    #[test]
    fn fetch_downloads_and_verifies() {
        crate::models::token_from(|| Some("secret".into()));
        let body: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        let (url, _, auth) = fake(body.clone(), true);
        let dest = temp("fetch").join("m").join("model.safetensors");
        let mut seen = 0;
        fetch(&agent(), &format!("{url}/x"), &dest, &spec(&body), &AtomicBool::new(false), "test", &mut |n| seen = n).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(seen, body.len() as u64);
        assert!(!part_of(&dest).exists());
        assert!(!auth.load(Ordering::SeqCst), "the token goes to huggingface.co only");
        assert!(is_hub("https://huggingface.co/BAAI/x/resolve/main/config.json") && !is_hub("https://huggingface.co.example.com/x"));
    }

    /// An interrupted download resumes from its `.part` with a range request and is still checked whole; a server
    /// that ignores ranges sends it all again, and that is right too.
    #[test]
    fn fetch_resumes_from_the_part_file() {
        let body: Vec<u8> = (0..45_000u32).map(|i| (i * 7 % 256) as u8).collect();
        for ranges in [true, false] {
            let (url, hits, _) = fake(body.clone(), ranges);
            let dest = temp(if ranges { "resume" } else { "no-ranges" }).join("model.safetensors");
            std::fs::write(part_of(&dest), &body[..12_345]).unwrap();
            let mut first = None;
            fetch(&agent(), &format!("{url}/x"), &dest, &spec(&body), &AtomicBool::new(false), "test", &mut |n| _ = first.get_or_insert(n)).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), body);
            assert_eq!(first, Some(if ranges { 12_345 } else { 0 }));
            assert_eq!(hits.load(Ordering::SeqCst), 1);
        }
    }

    /// A file whose checksum is not the one expected is refused and its `.part` deleted; a cancelled one keeps its
    /// `.part`. A small file is checked by its Git blob SHA-1.
    #[test]
    fn fetch_refuses_a_bad_checksum_and_stops_when_cancelled() {
        let body = b"{\"hidden_size\": 768}".to_vec();
        let (url, _, _) = fake(body.clone(), true);
        let dir = temp("checksum");
        let dest = dir.join("config.json");
        let mut git = sha1::Sha1::new();
        git.update(format!("blob {}\0", body.len()).as_bytes());
        git.update(&body);
        let good = FileSpec { name: "config.json".into(), bytes: body.len() as u64, sha256: None, git_sha1: Some(hex(&git.finalize())) };
        let bad = FileSpec { git_sha1: Some("0".repeat(40)), ..good.clone() };
        let e = fetch(&agent(), &format!("{url}/x"), &dest, &bad, &AtomicBool::new(false), "test", &mut |_| {}).unwrap_err();
        assert!(e.contains("checksum does not match") && !dest.exists() && !part_of(&dest).exists(), "{e}");
        fetch(&agent(), &format!("{url}/x"), &dest, &good, &AtomicBool::new(false), "test", &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let other = dir.join("cancelled.json");
        let e = fetch(&agent(), &format!("{url}/x"), &other, &good, &AtomicBool::new(true), "test", &mut |_| {}).unwrap_err();
        assert_eq!(e, CANCELLED);
        assert!(!other.exists() && part_of(&other).exists());
    }
}
