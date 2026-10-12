//! Plain-English progress text shared by download and ingest: sizes, durations, percentages.

pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

pub fn human_time(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600).div_ceil(60)),
    }
}

/// "release.tar.xz: 84% (18.5 of 22.0 GB), 21.4 MB/s, about 3 min left". `done` includes bytes
/// from earlier runs; `rate` is bytes per second of this run. Without a known total it just
/// says how much has arrived.
pub fn progress_line(name: &str, done: u64, total: Option<u64>, rate: f64) -> String {
    let speed = format!("{:.1} MB/s", rate / 1e6);
    match total {
        Some(total) if total > 0 => {
            let pct = (done as f64 / total as f64 * 100.0).min(100.0);
            let left = if rate > 0.0 {
                format!(
                    ", about {} left",
                    human_time((total.saturating_sub(done) as f64 / rate) as u64)
                )
            } else {
                String::new()
            };
            format!(
                "{name}: {pct:.0}% ({} of {}), {speed}{left}",
                human_size(done),
                human_size(total)
            )
        }
        _ => format!("{name}: {} downloaded, {speed}", human_size(done)),
    }
}

/// Time left from how far along we are. `fraction` is 0..1. Says nothing until there is enough
/// data to make the guess worth reading.
pub fn time_left(fraction: f64, elapsed_secs: f64) -> Option<String> {
    if fraction < 0.001 || elapsed_secs < 20.0 {
        return None;
    }
    let left = elapsed_secs * (1.0 - fraction.min(1.0)) / fraction;
    Some(human_time(left as u64))
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_says_percent_size_and_time_left() {
        // 18.5 GB of 22 GB done, 20 MB/s: 3.5 GB left is 175 s, shown as 3 min.
        let line =
            super::progress_line("release.tar.xz", 18_500_000_000, Some(22_000_000_000), 20e6);
        assert_eq!(
            line,
            "release.tar.xz: 84% (18.5 GB of 22.0 GB), 20.0 MB/s, about 3 min left"
        );
        // Unknown total: no percentage, no promise about time.
        let line = super::progress_line("artist.tar.xz", 500_000_000, None, 10e6);
        assert_eq!(line, "artist.tar.xz: 500 MB downloaded, 10.0 MB/s");
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(super::human_time(45), "45 s");
        assert_eq!(super::human_time(61), "2 min");
        assert_eq!(super::human_time(5400), "1 h 30 min");
    }

    #[test]
    fn time_left_waits_for_enough_data() {
        assert_eq!(super::time_left(0.0005, 100.0), None);
        assert_eq!(super::time_left(0.5, 5.0), None);
        // A quarter done after 10 minutes: 30 more minutes.
        assert_eq!(super::time_left(0.25, 600.0), Some("30 min".to_string()));
    }
}
