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
        // Dumps are tens of GB: no overall limit, but give up if the server goes quiet.
        .timeout_global(None)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
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

fn fetch_file(agent: &ureq::Agent, url: &str, dest: &Path, expected: &str) -> Result<()> {
    if dest.exists() {
        if sha256_file(dest)? == expected {
            eprintln!("{}: already downloaded and verified", dest.display());
            return Ok(());
        }
        fs::remove_file(dest)?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    for attempt in 0..2 {
        let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut res = req.call().with_context(|| format!("GET {url}"))?;
        let mut file = match res.status().as_u16() {
            206 => {
                eprintln!("{}: resuming at {} MB", dest.display(), have >> 20);
                OpenOptions::new().append(true).open(&part)?
            }
            200 => File::create(&part)?, // fresh download, or the server ignored the Range header
            416 if attempt == 0 => {
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
        copy_with_progress(res.body_mut().as_reader(), &mut file, dest)?;
        file.flush()?;
        drop(file);
        let got = sha256_file(&part)?;
        if got != expected {
            fs::remove_file(&part)?;
            bail!(
                "{}: checksum mismatch (expected {expected}, got {got}); deleted the download",
                dest.display()
            );
        }
        fs::rename(&part, dest)?;
        return Ok(());
    }
    bail!("{}: could not download", dest.display())
}

fn copy_with_progress(mut from: impl Read, to: &mut File, dest: &Path) -> Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    let (mut total, mut last, started) = (0u64, Instant::now(), Instant::now());
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        total += n as u64;
        if last.elapsed() > Duration::from_secs(5) {
            last = Instant::now();
            let rate = total as f64 / started.elapsed().as_secs_f64() / 1e6;
            eprintln!(
                "{}: {} MB this run ({rate:.1} MB/s)",
                dest.display(),
                total >> 20
            );
        }
    }
    Ok(())
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
    eprintln!("dump {date} is in {}", dir.display());
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
}
