use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tracing::info;

use crate::Progress;

pub const FLAGS: [&str; 2] = ["--no_block_detail", "--prog-second"];
const STATUS_PREFIX: &str = "Elapsed:";

pub async fn follow(stderr: impl AsyncRead + Unpin, on_progress: impl Fn(Progress)) {
    let mut segments = BufReader::new(stderr).split(b'\r');
    while let Ok(Some(segment)) = segments.next_segment().await {
        for line in String::from_utf8_lossy(&segment).lines().map(str::trim) {
            match parse(line) {
                Some(progress) => on_progress(progress),
                None if line.is_empty() || line.starts_with(STATUS_PREFIX) => {}
                None => info!("{line}"),
            }
        }
    }
}

fn parse(line: &str) -> Option<Progress> {
    let (_, fields) = line.strip_prefix(STATUS_PREFIX)?.split_once("Remaining:")?;
    let (remaining, fields) = fields.split_once(", Completed:")?;
    let (percent, rate) = fields.split_once("%,")?;
    let bytes_per_second = bytes_per_second(rate)?;
    if bytes_per_second == 0.0 {
        return Some(Progress::default());
    }
    Some(Progress {
        fraction: (percent.trim().parse::<f64>().ok()? / 100.0).clamp(0.0, 1.0),
        bytes_per_second,
        remaining: duration(remaining.trim()),
    })
}

fn bytes_per_second(rate: &str) -> Option<f64> {
    let rate = rate.trim().trim_end_matches(',');
    let rate = rate.strip_prefix("Rate:").unwrap_or(rate).trim();
    let amount = rate.strip_suffix("/sec")?;
    let (number, unit) = amount.split_at(amount.find(|c: char| c.is_ascii_alphabetic())?);
    let scale = match unit {
        "byte" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        _ => return None,
    };
    Some(number.trim().parse::<f64>().ok()? * scale)
}

fn duration(hours_minutes_seconds: &str) -> Option<Duration> {
    let mut parts = hours_minutes_seconds
        .split(':')
        .map(|part| part.parse::<u64>().ok());
    let (hours, minutes, seconds) = (parts.next()??, parts.next()??, parts.next()??);
    Some(Duration::from_secs(hours * 3600 + minutes * 60 + seconds))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const RESTORE_STDERR: &[u8] = b"Partclone v0.3.48 http://partclone.org\n\
Starting to restore image (-) to device (target.img)\n\
Calculating bitmap... Please wait...\n\
done!\n\
File system:  EXTFS\n\
Block size:   4096 Byte\n\
\r                                                                                \r\
Elapsed: 00:00:01, Remaining: 00:01:39, Completed:   1.00%,   0.00byte/sec,\
\r                                                                                \r\
Elapsed: 00:00:02, Remaining: 00:00:02, Completed:  37.50%, 106.50MB/sec,\
\r                                                                                \r\
Elapsed: 00:00:06, Remaining: 00:00:00, Completed: 100.00%, Rate:  78.25MB/sec,\n\
Total Time: 00:00:06, 100.00% completed!\n\
Syncing... OK!\n";

    const BITMAP_SCAN: &[u8] = b"Calculating bitmap... Please wait... \n\
\r                                                                                \r\
Elapsed: 00:00:01, Remaining: 00:01:39, Completed:   1.00%\
\r                                                                                \r\
Elapsed: 00:00:01, Remaining: 00:00:00, Completed: 100.00%\n\
Total Time: 00:00:01, 100.00% completed!\n\
done!\n";

    async fn followed(stderr: &[u8]) -> Vec<Progress> {
        let reported = Mutex::new(Vec::new());
        follow(stderr, |progress| reported.lock().unwrap().push(progress)).await;
        reported.into_inner().unwrap()
    }

    #[tokio::test]
    async fn every_data_status_line_is_reported_in_order() {
        let reported = followed(RESTORE_STDERR).await;

        assert_eq!(
            reported,
            [
                Progress::default(),
                Progress {
                    fraction: 0.375,
                    bytes_per_second: 106.5e6,
                    remaining: Some(Duration::from_secs(2)),
                },
                Progress {
                    fraction: 1.0,
                    bytes_per_second: 78.25e6,
                    remaining: Some(Duration::ZERO),
                },
            ]
        );
    }

    #[tokio::test]
    async fn the_bitmap_scan_before_the_data_copy_is_not_reported() {
        assert!(followed(BITMAP_SCAN).await.is_empty());
    }

    #[test]
    fn the_placeholder_before_any_data_is_copied_reports_nothing_done_and_no_eta() {
        let progress =
            parse("Elapsed: 00:00:01, Remaining: 00:01:39, Completed:   1.00%,   0.00byte/sec,");

        assert_eq!(progress, Some(Progress::default()));
    }

    #[test]
    fn a_remaining_time_beyond_a_day_is_unknown() {
        let progress =
            parse("Elapsed: 00:10:00, Remaining:  > 25 hrs, Completed:   0.50%,   1.25KB/sec,")
                .unwrap();

        assert_eq!(progress.remaining, None);
        assert_eq!(progress.bytes_per_second, 1250.0);
    }

    #[test]
    fn rates_scale_by_their_decimal_unit() {
        assert_eq!(bytes_per_second(" 512.00byte/sec,"), Some(512.0));
        assert_eq!(bytes_per_second("  1.50GB/sec,"), Some(1.5e9));
        assert_eq!(bytes_per_second("  1.50GB/min,"), None);
    }
}
