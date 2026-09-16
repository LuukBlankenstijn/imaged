pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds:02}s"),
        _ => format!("{hours}h {minutes:02}m"),
    }
}

#[cfg(feature = "web")]
fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(feature = "web"))]
fn now_millis() -> i64 {
    0
}

pub fn format_relative(millis: i64) -> String {
    let now = now_millis();
    if now == 0 {
        return String::new();
    }
    let diff = now - millis;
    let past = diff >= 0;
    let secs = diff.abs() / 1000;
    let (value, unit) = if secs < 60 {
        (secs, "s")
    } else if secs < 3600 {
        (secs / 60, "m")
    } else if secs < 86_400 {
        (secs / 3600, "h")
    } else {
        (secs / 86_400, "d")
    };
    if past {
        format!("{value}{unit} ago")
    } else {
        format!("in {value}{unit}")
    }
}
