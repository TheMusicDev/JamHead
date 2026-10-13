//! Fetch MusicBrainz JSON dumps: find the latest one, download the files we need, resume
//! interrupted downloads, and check every file against the published SHA256SUMS.
//!
//! Layout: `<out>/<dump date>/{artist,release-group,release}.tar.xz`, plus `SHA256SUMS` and a
//! `DUMP_DATE` file that ingest copies into the data directory's meta.json.
//!
//! Not done: the `.asc` PGP signatures are not checked. The checksums come from the same server
//! as the files, so they catch damaged or cut-off downloads but not a hostile server.

use crate::datadir::sha256_file;
use crate::store::Kind;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEFAULT_BASE_URL: &str = "https://data.metabrainz.org/pub/musicbrainz/data/json-dumps";

pub struct Options {
    pub base_url: String,
    /// "latest" or a dump directory name such as "20261010-001001".
    pub dump: String,
    pub kinds: Vec<Kind>,
    pub out: PathBuf,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Dumps are tens of GB, so there is no time limit on the body. `timeout_recv_body` caps the
        // WHOLE body (it is not restarted per read), so any value would kill a big download midway:
        // 120 s cut off the 22 GB release dump at about 2.3 GB. A broken connection is handled by
        // retrying and resuming instead (see `fetch_file`).
        // ponytail: a connection that stays open but sends nothing is not detected; Ctrl-C and rerun resumes.
        .timeout_global(None)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(None)
        .http_status_as_error(false)
        .build()
        .into()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    let mut res = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if res.status().as_u16() != 200 {
        bail!("GET {url}: HTTP {}", res.status());
    }
    Ok(res.body_mut().read_to_string()?)
}

/// Name of the newest dump directory, from the `LATEST` file.
pub fn latest(base_url: &str) -> Result<String> {
    let name = get_text(&agent(), &format!("{base_url}/LATEST"))?
        .trim()
        .to_string();
    if name.is_empty() || name.contains('/') {
        bail!("unexpected LATEST contents: {name:?}");
    }
    Ok(name)
}

/// "<hash>  <file name>" lines into a map.
fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.split_once(char::is_whitespace)?;
            Some((
                name.trim().trim_start_matches('*').to_string(),
                hash.to_string(),
            ))
        })
        .collect()
}

const MAX_TRIES: u32 = 8;
const RETRY_PAUSE: Duration = Duration::from_secs(5);

fn fetch_file(agent: &ureq::Agent, url: &str, dest: &Path, expected: &str) -> Result<()> {
    if dest.exists() {
        crate::log!(
            "{}: found a finished file, checking its checksum (a big file takes a few minutes)...",
            short(dest)
        );
        if sha256_file(dest)? == expected {
            crate::log!("{}: already downloaded and verified", short(dest));
            return Ok(());
        }
        fs::remove_file(dest)?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    // Each pass resumes from whatever is already in the .part file, so a broken connection
    // costs only the bytes since the last write.
    for attempt in 0..MAX_TRIES {
        let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut res = match req.call() {
            Ok(res) => res,
            // Right after a dropped connection the network may still be down: wait and try again.
            Err(e) if attempt + 1 < MAX_TRIES => {
                crate::log!(
                    "{}: {e}; try {} of {MAX_TRIES}, again in 5 s",
                    dest.display(),
                    attempt + 1
                );
                std::thread::sleep(RETRY_PAUSE);
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("GET {url}")),
        };
        let mut file = match res.status().as_u16() {
            206 => {
                crate::log!(
                    "{}: resuming from {} (a restarted download keeps what it already has)",
                    short(dest),
                    crate::progress::human_size(have)
                );
                OpenOptions::new().append(true).open(&part)?
            }
            200 => File::create(&part)?, // fresh download, or the server ignored the Range header
            416 => {
                // We already have all the bytes. If they check out we are done, else start over.
                if sha256_file(&part)? == expected {
                    fs::rename(&part, dest)?;
                    return Ok(());
                }
                fs::remove_file(&part)?;
                continue;
            }
            s => bail!("GET {url}: HTTP {s}"),
        };
        // The server says how many bytes are coming; add what we already have for the full size.
        let total = res
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|len| {
                if res.status().as_u16() == 206 {
                    have + len
                } else {
                    len
                }
            });
        let start = if res.status().as_u16() == 206 {
            have
        } else {
            0
        };
        if let Err(e) =
            copy_with_progress(res.body_mut().as_reader(), &mut file, dest, start, total)
        {
            crate::log!(
                "{}: connection broke ({e:#}); try {} of {MAX_TRIES}, resuming in 5 s",
                dest.display(),
                attempt + 1
            );
            drop(file);
            std::thread::sleep(RETRY_PAUSE);
            continue;
        }
        file.flush()?;
        drop(file);
        crate::log!(
            "{}: downloaded, checking its checksum (a big file takes a few minutes)...",
            short(dest)
        );
        let got = sha256_file(&part)?;
        if got != expected {
            fs::remove_file(&part)?;
            bail!(
                "{}: checksum mismatch (expected {expected}, got {got}); deleted the download",
                dest.display()
            );
        }
        fs::rename(&part, dest)?;
        crate::log!("{}: checksum OK", short(dest));
        return Ok(());
    }
    bail!("{}: could not download", dest.display())
}

/// Copy the body to the file, printing a progress line every 5 seconds. `start` is how many bytes
/// were already on disk, and `total` the full file size when the server told us.
fn copy_with_progress(
    mut from: impl Read,
    to: &mut File,
    dest: &Path,
    start: u64,
    total: Option<u64>,
) -> Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    let (mut copied, mut last, started) = (0u64, Instant::now(), Instant::now());
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        copied += n as u64;
        if last.elapsed() > Duration::from_secs(5) {
            last = Instant::now();
            let rate = copied as f64 / started.elapsed().as_secs_f64();
            crate::log!(
                "{}",
                crate::progress::progress_line(&short(dest), start + copied, total, rate)
            );
        }
    }
    Ok(())
}

/// Just the file name, for readable log lines.
fn short(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Download the requested dumps and return the directory they are in.
pub fn run(opts: &Options) -> Result<PathBuf> {
    let agent = agent();
    let base = opts.base_url.trim_end_matches('/');
    let date = if opts.dump == "latest" {
        latest(base)?
    } else {
        opts.dump.clone()
    };
    let dir = opts.out.join(&date);
    fs::create_dir_all(&dir)?;
    let sums_text = get_text(&agent, &format!("{base}/{date}/SHA256SUMS"))?;
    fs::write(dir.join("SHA256SUMS"), &sums_text)?;
    let sums = parse_sums(&sums_text);
    for kind in &opts.kinds {
        let file = format!("{}.tar.xz", kind.api_name());
        let expected = sums
            .get(&file)
            .with_context(|| format!("{file} is not listed in SHA256SUMS"))?;
        fetch_file(
            &agent,
            &format!("{base}/{date}/{file}"),
            &dir.join(&file),
            expected,
        )?;
    }
    fs::write(dir.join("DUMP_DATE"), &date)?;
    crate::log!("dump {date} is in {}", dir.display());
    Ok(dir)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sums_parse_both_checksum_file_styles() {
        let m = super::parse_sums("abc123  artist.tar.xz\ndef456 *release.tar.xz\n\n");
        assert_eq!(m["artist.tar.xz"], "abc123");
        assert_eq!(m["release.tar.xz"], "def456");
    }

    #[test]
    fn a_big_download_is_never_cut_off_by_a_time_limit() {
        // The body timeout covers the whole body, so ANY value kills a long download (it once cut
        // the 22 GB release dump off after 2 minutes).
        let agent = super::agent();
        assert!(agent.config().timeouts().recv_body.is_none());
        assert!(agent.config().timeouts().global.is_none());
    }
}
