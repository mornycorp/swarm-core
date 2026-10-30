//! UTC times as the tracking format writes them, `2026-10-18T06:49:51Z`, without a date crate.

/// `secs` since the epoch as `YYYY-MM-DDTHH:MM:SSZ` (the tracking format).
pub fn iso_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Now, as [`iso_utc`] writes it.
pub fn now_iso() -> String {
    iso_utc(now_secs())
}

/// Seconds since the epoch, now (0 for a clock before it).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `2026-10-18T06:49:51Z` as seconds since the Unix epoch (a fraction of a second, or a
/// `+hh:mm`/`-hh:mm` offset, is taken too; anything else is `None`). The inverse of [`iso_utc`].
pub fn parse_time(t: &str) -> Option<i64> {
    let (date, rest) = t.trim().split_once('T')?;
    let days = days_of(date)?;
    let (clock, offset) = clock_and_offset(rest)?;
    let clock = clock.split_once('.').map_or(clock, |(c, frac)| if num(frac).is_some() { c } else { "" });
    let mut c = clock.splitn(3, ':');
    let (hh, mm, ss) = (num(c.next()?)?, num(c.next()?)?, num(c.next()?)?);
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss - offset)
}

/// All ASCII digits, as a number.
fn num(s: &str) -> Option<i64> {
    s.bytes().all(|b| b.is_ascii_digit()).then(|| s.parse().ok()).flatten()
}

/// `YYYY-MM-DD` as days from 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_of(date: &str) -> Option<i64> {
    let mut d = date.splitn(3, '-');
    let (y, m, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || date.len() != 10 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The clock and the offset in seconds of what follows the `T`: `06:49:51Z`, `07:49:51+01:00`.
fn clock_and_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(c) = rest.strip_suffix('Z') {
        return Some((c, 0));
    }
    let at = rest.rfind(['+', '-'])?;
    let (c, o) = rest.split_at(at);
    let (oh, om) = o.get(1..)?.split_once(':')?;
    let secs = num(oh)? * 3600 + num(om)? * 60;
    Some((c, if o.starts_with('-') { -secs } else { secs }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_utc_formats_the_tracking_time() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_792_238_400), "2026-10-17T12:00:00Z");
        assert!(iso_utc(u64::MAX).ends_with('Z'), "no overflow on a hostile clock");
    }

    #[test]
    fn times_parse_as_iso_utc_writes_them() {
        for secs in [0u64, 1_792_224_000, 1_792_310_400 + 86_399, 951_782_400] {
            let iso = iso_utc(secs);
            assert_eq!(parse_time(&iso), Some(secs as i64), "{iso}");
        }
        assert_eq!(parse_time("2026-10-18T06:49:51.250Z"), parse_time("2026-10-18T06:49:51Z"));
        assert_eq!(parse_time("2026-10-18T07:49:51+01:00"), parse_time("2026-10-18T06:49:51Z"));
        assert_eq!(parse_time("2026-10-17T23:49:51-07:00"), parse_time("2026-10-18T06:49:51Z"));
        for bad in [
            "",
            "2026-10-18",
            "2026-13-01T00:00:00Z",
            "2026-10-18T24:00:00Z",
            "2026-10-18T06:49Z",
            "x026-10-18T06:49:51Z",
            "2026-10-18T06:49:51",
            "2026-10-18T06:49:51.xZ",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }
}
